// kestra: core/src/main/java/io/kestra/core/runners/pebble/AbstractDate.java
// java: java.time.format.DateTimeFormatterBuilder#parsePattern
// java: java.time.ZonedDateTime, java.time.ZoneId, java.time.temporal.WeekFields

//! W2 Task 5 — Java's `DateTimeFormatter` pattern language, and the
//! `ZonedDateTime` it formats.
//!
//! `{{ d | date("yyyy-MM-dd'T'HH:mm:ss") }}` is a
//! `java.time.format.DateTimeFormatter` pattern. It is **not** strftime and
//! not chrono's syntax: pattern letters repeat to select a width or a text
//! style (`M`, `MM`, `MMM`, `MMMM`, `MMMMM` are five different things), single
//! quotes fence literal text, `''` is one literal quote, and `[`/`]` open an
//! optional section. No Rust crate provides it, so this module does.
//!
//! # Why this formats directly instead of emitting a chrono format string
//!
//! The task brief describes this module as translating the pattern *to
//! chrono's*. It does not, and the reason is measurable rather than
//! stylistic: chrono's `strftime` has no spelling for `k` (clock-hour 1-24),
//! `K` (hour 0-11), `MMMMM` (narrow month), `X` (offset with `Z` for zero),
//! `ZZZZ`/`O` (localized GMT offset), `VV` (zone id), `G` (era), `Q`/`q`
//! (quarter), `W`/`F` (week-of-month), `e`/`c` (localized day-of-week), `Y`
//! (week-based year), `A`/`N` (milli/nano-of-day), `g` (modified julian day),
//! or `u` versus `y` (proleptic year versus year-of-era). A translation would
//! have to fall back to formatting those itself for two thirds of the
//! alphabet, so this walks the parsed pattern once and writes the output.
//! [`Pattern::parse`] is still a translation in the sense that matters: the
//! pattern is parsed **once, into a checked structure**, and Java's own
//! `IllegalArgumentException` messages are reproduced for the counts it
//! rejects.
//!
//! # What is deliberately NOT ported: text that lives in a locale database
//!
//! **Three** pattern letters and two of the four `FormatStyle`s need CLDR's
//! zone and day-period text, which is a data set egret does not carry and
//! cannot approximate without answering *plausibly wrong*:
//!
//! - **`z`/`zz`/`zzz`/`zzzz`** — the zone's *specific* display name. Measured
//!   through the oracle: `Asia/Tokyo` is `JST`, `Asia/Ho_Chi_Minh` is
//!   `GMT+07:00`, `Australia/Eucla` is `GMT+08:45`, `Europe/Moscow` is
//!   `GMT+03:00`. tzdb's own abbreviations (which `chrono-tz` exposes) agree
//!   for the first and the third but say `+07` for the second and `MSK` for
//!   the fourth. A rule that is right for some zones and silently wrong for
//!   others is worse here than a refusal, so [`Pattern::format`] refuses.
//! - **`v`/`vvvv`** — the zone's *generic* name (`JT` / `Japan Time`), a
//!   third CLDR table. Same reason.
//! - **`B`** — the day period as text (`in the morning`). Same reason.
//! - **`full` and `long`** — their CLDR patterns end in `zzzz` and `z`, so
//!   they inherit the refusal.
//!
//! Everything else in the alphabet is arithmetic or a fixed English word and
//! is ported. See [`Locale`] for the one other locale-shaped limit.
//!
//! **The PARSE side refuses the same three and no others**, deliberately: a
//! letter egret can write, it can read. Kestra parses all three (it consumes
//! and discards them), so those three are divergences on that side rather
//! than on both. Everything else a pattern can contain is parseable, including
//! the fields Java itself parses and then ignores.
//!
//! **This list is the whole refusal set, and it is now checked rather than
//! recited.** The fix round's probe sweep pushes every ASCII letter at counts
//! 1-11 through the real engine and through this module and differences the
//! two, so a letter that is refused here but valid in Java shows up as a diff.
//! `v` was missing from [`letter_limits`] entirely until it did: egret said
//! `Unknown pattern letter: v` where Java prints `Japan Time`, which tells a
//! flow author their pattern is invalid when it is not — a worse failure than
//! refusing it. **A letter added to the refusal set must be added here, given
//! a baselined corpus entry, and named in `oracle/README.md`**, the way `z`
//! was and `v` was not.

use std::fmt;
use std::fmt::Write as _;
use std::str::FromStr;

use chrono::{
    DateTime, Datelike, Days, FixedOffset, LocalResult, Months, NaiveDate, NaiveDateTime,
    NaiveTime, Offset, TimeZone, Timelike, Utc,
};
use chrono_tz::Tz;

// ---------------------------------------------------------------------------
// Locale
// ---------------------------------------------------------------------------

/// The locale `AbstractDate.format` resolves — `LocaleUtils.toLocale(arg)`, or
/// the `EvaluationContext`'s own locale when no `locale` argument is given.
///
/// **egret is English-only, and refuses rather than approximating.** Kestra's
/// month names, day names, era names and quarter names come from the JVM's
/// CLDR data for whatever locale is in force. egret carries the English names
/// and nothing else, so a `locale` argument naming another language is an
/// explicit refusal: answering `January` for `locale='fr'` would be a silently
/// wrong string in a renderer whose whole purpose is byte fidelity.
///
/// **The default is English too, and that is a second divergence.** Kestra's
/// default is `Locale.getDefault()` — on the machine that generates
/// `oracle/golden.json` that is `en_VN`, measured, and it is pinned in
/// `oracle/README.md`. Every corpus entry whose output depends on the locale
/// passes `locale='en'` explicitly for exactly that reason.
///
/// **Only `en` and `en_US` are accepted, not every English tag**, because
/// `WeekFields.of(locale)` is resolved from the locale's *country*: `en` and
/// `en_US` are Sunday-first with one minimal day, `en_GB` is Monday-first with
/// four, and `en_VN` is Monday-first with one. Measured directly against the
/// JDK. Accepting `en_GB` would answer `w`, `W`, `Y`, `e` and `c` wrongly
/// while looking supported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Locale;

impl Locale {
    /// `LocaleUtils.toLocale(spec)` followed by egret's own English-only gate.
    ///
    /// The first error is Kestra's own wording (commons-lang3 throws
    /// `IllegalArgumentException: Invalid locale format: <spec>` for a string
    /// that is not a locale at all); the second is egret's, and says so.
    pub(super) fn resolve(spec: &str) -> Result<Self, String> {
        if !is_locale_shaped(spec) {
            return Err(format!("Invalid locale format: {spec}"));
        }
        if spec == "en" || spec == "en_US" {
            return Ok(Locale);
        }
        Err(format!(
            "the 'date' filters support locale 'en' and 'en_US' only; '{spec}' would need a \
             locale database egret does not carry"
        ))
    }
}

/// `LocaleUtils.toLocale`'s shape check, narrowed to the forms a Kestra
/// expression can plausibly pass: `ll`, `ll_CC`, `ll_CC_variant`.
fn is_locale_shaped(spec: &str) -> bool {
    let mut parts = spec.split('_');
    let Some(language) = parts.next() else {
        return false;
    };
    if !(language.len() == 2 || language.len() == 3)
        || !language.chars().all(|c| c.is_ascii_lowercase())
    {
        return false;
    }
    match parts.next() {
        None => true,
        Some(country) => {
            country.len() == 2 && country.chars().all(|c| c.is_ascii_uppercase())
                || country.len() == 3 && country.chars().all(|c| c.is_ascii_digit())
        }
    }
}

// ---------------------------------------------------------------------------
// Zones
// ---------------------------------------------------------------------------

/// `ZoneId.of(id)` — either an IANA region or a fixed offset.
///
/// The two are kept apart because they answer `VV` (the zone id) differently
/// and because only a region has transitions: `ZonedDateTime.plus` on a
/// date-based unit re-resolves the local time against the region's rules, and
/// that is what makes `+ 1 DAY` across a DST boundary keep its wall clock
/// while `+ 24 HOURS` does not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Zone {
    /// An IANA region id, e.g. `Asia/Tokyo`.
    Region(Tz),
    /// A `ZoneOffset` (`Z`, `+05:30`) or one of Java's prefixed-offset
    /// regions (`GMT+7`, whose `getId()` is `GMT+07:00`). Both have constant
    /// rules; the string is the id `VV` prints.
    Fixed(FixedOffset, String),
}

impl Zone {
    /// `ZoneId.of(zone)`, or `ZoneId.systemDefault()` when Kestra was given no
    /// `timeZone` argument.
    ///
    /// **egret's default is UTC where Kestra's is the host's zone.** Kestra
    /// reads `ZoneId.systemDefault()`, which makes every unqualified `date`
    /// call answer differently on a developer's laptop and on a cluster node —
    /// `oracle/README.md` records that as a standing corpus rule (every date
    /// entry passes `timeZone` explicitly) and it is a worse default for a
    /// server that renders other people's workflows. Deliberate, and the only
    /// divergence in this function.
    pub(super) fn resolve(id: Option<&str>) -> Result<Self, String> {
        let Some(id) = id else {
            return Ok(Zone::Fixed(
                FixedOffset::east_opt(0).expect("0 is a valid offset"),
                "Z".to_string(),
            ));
        };
        if id == "Z" {
            return Ok(Zone::Fixed(
                FixedOffset::east_opt(0).expect("0 is a valid offset"),
                "Z".to_string(),
            ));
        }
        if id.starts_with('+') || id.starts_with('-') {
            let offset = parse_offset_id(id)?;
            return Ok(Zone::Fixed(offset, offset_id(offset)));
        }
        for prefix in ["UTC", "GMT", "UT"] {
            if let Some(rest) = id.strip_prefix(prefix) {
                if rest.is_empty() {
                    // `ZoneId.of("UTC")`/`("GMT")` are regions with those ids
                    // and constant rules; `UT` is the same shape but has no
                    // tzdb entry, so all three are built here rather than
                    // looked up.
                    return Ok(Zone::Fixed(
                        FixedOffset::east_opt(0).expect("0 is a valid offset"),
                        prefix.to_string(),
                    ));
                }
                if rest.starts_with('+') || rest.starts_with('-') {
                    // `ZoneId.of` re-wraps every `ZoneOffset` failure under a
                    // prefix, so `GMT+` is `Invalid ID for offset-based
                    // ZoneId` and not the offset parser's own sentence.
                    let offset = parse_offset_id(rest)
                        .map_err(|_| format!("Invalid ID for offset-based ZoneId: {id}"))?;
                    let suffix = offset_id(offset);
                    let name = if suffix == "Z" {
                        prefix.to_string()
                    } else {
                        format!("{prefix}{suffix}")
                    };
                    return Ok(Zone::Fixed(offset, name));
                }
            }
        }
        // **Four ids `chrono-tz` carries that `ZoneId.of` rejects.** The JDK's
        // tzdb build drops the three-letter `backward` links that collide with
        // `ZoneId.SHORT_IDS`, plus `ROC`. The set is CLOSED, not a guess: every
        // one of `chrono-tz` 0.10's 597 zone names was fed to `ZoneId.of` and
        // exactly these four threw. (The other direction has 13 entries, all
        // `SystemV/*`, which `chrono-tz` does not carry at all -- recorded in
        // `oracle/README.md`, not fixable here.)
        if matches!(id, "EST" | "HST" | "MST" | "ROC") {
            return Err(format!("Unknown time-zone ID: {id}"));
        }
        // `Tz`'s parser is not case-sensitive about every id, and Java's is:
        // `ZoneId.of("utc")` throws. Requiring the canonical spelling back
        // keeps the two agreeing about what is a zone.
        match Tz::from_str(id) {
            Ok(tz) if tz.name() == id => Ok(Zone::Region(tz)),
            _ => Err(format!("Unknown time-zone ID: {id}")),
        }
    }

    /// What `VV` prints: `ZoneId.getId()`.
    pub(super) fn id(&self) -> String {
        match self {
            Zone::Region(tz) => tz.name().to_string(),
            Zone::Fixed(_, id) => id.clone(),
        }
    }

    fn offset_at(&self, instant: DateTime<Utc>) -> FixedOffset {
        match self {
            Zone::Region(tz) => tz.offset_from_utc_datetime(&instant.naive_utc()).fix(),
            Zone::Fixed(offset, _) => *offset,
        }
    }

    /// `ZonedDateTime.ofLocal(local, zone, null)` — resolve a wall clock
    /// against this zone's rules.
    ///
    /// Java's three cases, reproduced: a normal local time keeps its offset,
    /// an **overlap** (the hour repeated when a zone falls back) takes the
    /// *earlier* offset, and a **gap** (the hour skipped when a zone springs
    /// forward) moves the local time forward by the gap's own duration and
    /// takes the offset after it — so 02:30 on a spring-forward day becomes
    /// 03:30, not 03:00.
    fn resolve_local(&self, local: NaiveDateTime) -> (NaiveDateTime, FixedOffset) {
        match self {
            Zone::Fixed(offset, _) => (local, *offset),
            Zone::Region(tz) => match tz.from_local_datetime(&local) {
                // `Ambiguous` is Java's overlap, and chrono's first value is
                // the earlier offset -- which is the one Java keeps.
                LocalResult::Single(dt) => (local, dt.offset().fix()),
                LocalResult::Ambiguous(earlier, _) => (local, earlier.offset().fix()),
                LocalResult::None => {
                    // A gap. `chrono-tz` exposes no transition object, so the
                    // gap's duration is read off the offsets a day either
                    // side of it -- exact for every tzdb transition, which
                    // are never less than a day apart.
                    let before = self.offset_at(Utc.from_utc_datetime(&(local - Days::new(1))));
                    let after = self.offset_at(Utc.from_utc_datetime(&(local + Days::new(1))));
                    let gap = i64::from(after.local_minus_utc() - before.local_minus_utc());
                    let shifted = local + chrono::TimeDelta::seconds(gap);
                    (shifted, after)
                }
            },
        }
    }
}

/// `ZoneOffset.of(id)`, whose grammar is driven by the id's **LENGTH**, not by
/// how many digits it happens to contain.
///
/// `ZoneOffset.of` switches on `offsetId.length()` and reads each number from
/// a fixed position: 2 is `+h`, 3 is `+hh`, 5 is `+hhmm`, 6 is `+hh:mm`, 7 is
/// `+hhmmss`, 9 is `+hh:mm:ss`, and every other length is rejected outright.
/// A first cut of this counted digits and ignored colon *placement*, which
/// accepted `+0:730` and `+5:30` — both of which Java rejects, the first two
/// with `non numeric characters found` because position 1 is a digit position
/// and holds `0:` and `5:`. Found by the fix round's probe sweep.
///
/// Each rejection carries Java's own sentence, because these reach a flow
/// author as the message on their `timeZone` argument.
fn parse_offset_id(id: &str) -> Result<FixedOffset, String> {
    let invalid = || format!("Invalid ID for ZoneOffset, invalid format: {id}");
    let non_numeric = || format!("Invalid ID for ZoneOffset, non numeric characters found: {id}");
    let no_colon = || format!("Invalid ID for ZoneOffset, colon not found when expected: {id}");

    let bytes = id.as_bytes();
    let sign = match bytes.first() {
        Some(b'+') => 1i32,
        Some(b'-') => -1i32,
        _ => return Err(invalid()),
    };
    // Length 2 is `+h`, which Java handles by rewriting it to `+0h` and
    // falling through to the length-3 arm. That is what makes
    // `ZoneId.of("GMT+7")` a zone while `ZoneOffset.of("+5:30")` is not one.
    let padded;
    let (bytes, len) = if id.len() == 2 {
        padded = format!("{}0{}", &id[..1], &id[1..]);
        (padded.as_bytes(), 3)
    } else {
        (bytes, id.len())
    };
    let number = |pos: usize, preceded_by_colon: bool| -> Result<i32, String> {
        if preceded_by_colon && bytes.get(pos - 1) != Some(&b':') {
            return Err(no_colon());
        }
        let hi = bytes.get(pos).copied().ok_or_else(non_numeric)?;
        let lo = bytes.get(pos + 1).copied().ok_or_else(non_numeric)?;
        if !hi.is_ascii_digit() || !lo.is_ascii_digit() {
            return Err(non_numeric());
        }
        Ok(i32::from((hi - b'0') * 10 + (lo - b'0')))
    };
    let (h, m, s) = match len {
        3 => (number(1, false)?, 0, 0),
        5 => (number(1, false)?, number(3, false)?, 0),
        6 => (number(1, false)?, number(4, true)?, 0),
        7 => (number(1, false)?, number(3, false)?, number(5, false)?),
        9 => (number(1, false)?, number(4, true)?, number(7, true)?),
        _ => return Err(invalid()),
    };

    // `ZoneOffset.validate`, in its own order: hours, then minutes, then the
    // total. Each has a different sentence and the sweep separates them.
    if !(-18..=18).contains(&h) {
        return Err(format!(
            "Zone offset hours not in valid range: value {h} is not in the range -18 to 18"
        ));
    }
    if !(0..=59).contains(&m) {
        return Err(format!(
            "Zone offset minutes not in valid range: value {m} is not in the range -59 to 59"
        ));
    }
    let total = sign * (h * 3600 + m * 60 + s);
    if total.abs() > 18 * 3600 {
        return Err("Zone offset not in valid range: -18:00 to +18:00".to_string());
    }
    FixedOffset::east_opt(total).ok_or_else(invalid)
}

/// `ZoneOffset.getId()`: `Z` for zero, else `+HH:MM[:SS]`.
fn offset_id(offset: FixedOffset) -> String {
    let total = offset.local_minus_utc();
    if total == 0 {
        return "Z".to_string();
    }
    let sign = if total < 0 { '-' } else { '+' };
    let abs = total.abs();
    let (h, m, s) = (abs / 3600, (abs % 3600) / 60, abs % 60);
    if s == 0 {
        format!("{sign}{h:02}:{m:02}")
    } else {
        format!("{sign}{h:02}:{m:02}:{s:02}")
    }
}

// ---------------------------------------------------------------------------
// The value being formatted
// ---------------------------------------------------------------------------

/// A `java.time.ZonedDateTime`: a wall clock, the offset in force, and the
/// zone that produced it.
///
/// Kept as the local time rather than as an instant because that is what Java
/// carries and what its date-based arithmetic operates on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Zdt {
    local: NaiveDateTime,
    offset: FixedOffset,
    zone: Zone,
}

impl Zdt {
    pub(super) fn new(local: NaiveDateTime, zone: Zone) -> Self {
        let (local, offset) = zone.resolve_local(local);
        Zdt {
            local,
            offset,
            zone,
        }
    }

    fn from_instant(instant: DateTime<Utc>, zone: Zone) -> Self {
        let offset = zone.offset_at(instant);
        Zdt {
            local: instant.naive_utc()
                + chrono::TimeDelta::seconds(i64::from(offset.local_minus_utc())),
            offset,
            zone,
        }
    }

    /// `Instant.ofEpochSecond(seconds, nanos).atZone(zone)`.
    pub(super) fn from_epoch(seconds: i64, nanos: u32, zone: Zone) -> Option<Self> {
        let instant = DateTime::from_timestamp(seconds, nanos)?;
        Some(Zdt::from_instant(instant, zone))
    }

    fn instant(&self) -> DateTime<Utc> {
        Utc.from_utc_datetime(
            &(self.local - chrono::TimeDelta::seconds(i64::from(self.offset.local_minus_utc()))),
        )
    }

    /// `DateTimeFormatter.withZone(zone).format(this)` — the same instant seen
    /// from another zone, which is what every `date`/`dateAdd` render does
    /// just before writing.
    pub(super) fn with_zone(&self, zone: &Zone) -> Self {
        Zdt::from_instant(self.instant(), zone.clone())
    }

    /// `ZonedDateTime.toEpochSecond()`.
    pub(super) fn epoch_second(&self) -> i64 {
        self.instant().timestamp()
    }

    /// `ZonedDateTime.getNano()`.
    ///
    /// Read straight off the local time, the way [`format_field`] does. A
    /// first cut went via `and_utc().timestamp_subsec_nanos()`, which is the
    /// same number by a longer route and left two spellings of one field in
    /// the module.
    pub(super) fn nano(&self) -> u32 {
        self.local.nanosecond().min(999_999_999)
    }

    /// `ZonedDateTime.plus(amount, unit)`.
    ///
    /// **The split between the two arms is Java's `unit.isDateBased()`, and it
    /// changes answers across a DST boundary.** A date-based unit is added to
    /// the *local* date-time and the result re-resolved against the zone, so
    /// `+ 1 DAYS` keeps the wall clock; a time-based unit is added to the
    /// *instant*, so `+ 24 HOURS` does not. Measured through the oracle on
    /// `America/New_York` either side of both 2024 transitions.
    pub(super) fn plus(&self, amount: i64, unit: ChronoUnit) -> Result<Self, String> {
        if unit.is_date_based() {
            let local = unit.add_to_local(self.local, amount)?;
            Ok(Zdt::new(local, self.zone.clone()))
        } else {
            let delta = unit.to_time_delta(amount)?;
            let instant = self
                .instant()
                .checked_add_signed(delta)
                .ok_or_else(|| out_of_chrono_range(delta.num_days() / 366))?;
            Ok(Zdt::from_instant(instant, self.zone.clone()))
        }
    }
}

/// `java.time.temporal.ChronoUnit`, the enum `dateAdd`'s `unit` argument
/// names. `ChronoUnit.valueOf` is exact and case-sensitive, so `'days'` is an
/// error where `'DAYS'` is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChronoUnit {
    Nanos,
    Micros,
    Millis,
    Seconds,
    Minutes,
    Hours,
    HalfDays,
    Days,
    Weeks,
    Months,
    Years,
    Decades,
    Centuries,
    Millennia,
    Eras,
    Forever,
}

impl ChronoUnit {
    /// `ChronoUnit.valueOf(name)`, reproducing `java.lang.IllegalArgumentException`'s
    /// wording for a name that is not one of the sixteen.
    pub(super) fn resolve(name: &str) -> Result<Self, String> {
        Ok(match name {
            "NANOS" => ChronoUnit::Nanos,
            "MICROS" => ChronoUnit::Micros,
            "MILLIS" => ChronoUnit::Millis,
            "SECONDS" => ChronoUnit::Seconds,
            "MINUTES" => ChronoUnit::Minutes,
            "HOURS" => ChronoUnit::Hours,
            "HALF_DAYS" => ChronoUnit::HalfDays,
            "DAYS" => ChronoUnit::Days,
            "WEEKS" => ChronoUnit::Weeks,
            "MONTHS" => ChronoUnit::Months,
            "YEARS" => ChronoUnit::Years,
            "DECADES" => ChronoUnit::Decades,
            "CENTURIES" => ChronoUnit::Centuries,
            "MILLENNIA" => ChronoUnit::Millennia,
            "ERAS" => ChronoUnit::Eras,
            "FOREVER" => ChronoUnit::Forever,
            other => {
                return Err(format!(
                    "No enum constant java.time.temporal.ChronoUnit.{other}"
                ))
            }
        })
    }

    fn is_date_based(self) -> bool {
        matches!(
            self,
            ChronoUnit::Days
                | ChronoUnit::Weeks
                | ChronoUnit::Months
                | ChronoUnit::Years
                | ChronoUnit::Decades
                | ChronoUnit::Centuries
                | ChronoUnit::Millennia
                | ChronoUnit::Eras
                | ChronoUnit::Forever
        )
    }

    /// The duration `LocalDateTime.plus(amount, unit)` actually adds.
    ///
    /// # `HALF_DAYS` is not twelve hours per unit, and the JDK says so in a
    /// comment that is wrong
    ///
    /// `LocalDateTime.plus` is
    /// `case HALF_DAYS -> plusDays(amountToAdd / 256).plusHours((amountToAdd %
    /// 256) * 12);` — read out of `src.zip` on the same Corretto 25 the
    /// goldens are generated with, and annotated there
    /// `// no overflow (256 is multiple of 2)`. The divisor should be 2. It is
    /// 256, so the identity `n half-days == n * 12h` holds only for
    /// `|n| < 256`: at 256 the JDK adds **one day**, not 128, and
    /// `dateAdd(-1000, 'HALF_DAYS')` is 119 days back rather than 500.
    ///
    /// Reproduced rather than corrected. This is a wrong *answer*, not a
    /// resource vector, and a port that quietly fixed it would give a
    /// different date for the same flow — which is the one thing this package
    /// exists to prevent. Found by the fix round's probe sweep, which walked
    /// every `ChronoUnit` at ten magnitudes; the original corpus probed
    /// `HALF_DAYS` only at 1, where the two agree.
    ///
    /// Every other time-based unit is exact: `MICROS` and `MILLIS` use the
    /// same split-the-days trick with the *correct* divisors
    /// (`MICROS_PER_DAY`, `MILLIS_PER_DAY`), so they cancel.
    /// # The amount is split into DAYS plus a remainder, and that is not a
    /// micro-optimisation
    ///
    /// `LocalDateTime.plusSeconds` and friends are `plusWithOverflow(date,
    /// …)`, which divides by the unit's count-per-day **before** multiplying
    /// anything. A first cut multiplied the whole amount into nanoseconds, so
    /// an `i64` overflowed at about ±292 years and
    /// `dateAdd(10000000000, 'SECONDS')` said `long overflow` where Kestra
    /// answers `2340-12-05` — a date **inside** what egret can represent. Six
    /// of the sixteen units cliffed 900× short of the limit their own error
    /// message claimed, and the message was Java's arithmetic-overflow
    /// sentence for a case Java's arithmetic does not overflow on. Found by
    /// the third review.
    fn to_time_delta(self, amount: i64) -> Result<chrono::TimeDelta, String> {
        let overflow = || "long overflow".to_string();
        // (per-day, nanos-per-unit) — the divisor comes first, exactly as
        // `plusWithOverflow` does it.
        let (per_day, nanos_each): (i64, i64) = match self {
            ChronoUnit::Nanos => (86_400 * 1_000_000_000, 1),
            ChronoUnit::Micros => (86_400 * 1_000_000, 1_000),
            ChronoUnit::Millis => (86_400 * 1_000, 1_000_000),
            ChronoUnit::Seconds => (86_400, 1_000_000_000),
            ChronoUnit::Minutes => (1_440, 60 * 1_000_000_000),
            ChronoUnit::Hours => (24, 3_600 * 1_000_000_000),
            // `HALF_DAYS` first goes through the JDK's own `/ 256` defect (see
            // this type's doc), which yields a whole number of hours.
            ChronoUnit::HalfDays => {
                let hours = (amount / 256)
                    .checked_mul(24)
                    .and_then(|h| h.checked_add((amount % 256).checked_mul(12)?))
                    .ok_or_else(overflow)?;
                return delta_from(hours / 24, (hours % 24) * 3_600 * 1_000_000_000);
            }
            _ => unreachable!("only time-based units reach to_time_delta"),
        };
        delta_from(amount / per_day, (amount % per_day) * nanos_each)
    }

    fn add_to_local(self, local: NaiveDateTime, amount: i64) -> Result<NaiveDateTime, String> {
        let overflow = || "long overflow".to_string();
        let range = |years: i64| out_of_chrono_range(years);
        let out_of_range = |years: i64| {
            format!("Invalid value for Year (valid values -999999999 - 999999999): {years}")
        };
        match self {
            ChronoUnit::Forever => Err("Unsupported unit: Forever".to_string()),
            ChronoUnit::Eras => {
                // `LocalDate.with(ERA, era + amount)`. ISO has exactly two
                // eras, so 0 and 1 are the only valid targets -- but the valid
                // ones are not no-ops: `with(ERA, 0)` on 2024 is
                // `withYear(1 - 2024)`, i.e. -2023. A first cut errored on
                // every non-identity shift, which made `dateAdd(-1, 'ERAS')`
                // an error where Kestra answers `-2023-01-15`. Found by the
                // fix round's probe sweep.
                let era = i64::from(local.year() > 0);
                let target = era.checked_add(amount).ok_or_else(overflow)?;
                if target == era {
                    return Ok(local);
                }
                if !(0..=1).contains(&target) {
                    return Err(format!(
                        "Invalid value for Era (valid values 0 - 1): {target}"
                    ));
                }
                let year = 1 - local.year();
                local
                    .with_year(year)
                    .ok_or_else(|| out_of_chrono_range(i64::from(year)))
            }
            // `ok_or_else(range)`, NOT `overflow`: reaching the end of what a
            // `NaiveDateTime` can hold is egret's limit, not Java's `long`
            // arithmetic overflowing. See [`out_of_chrono_range`].
            // Java overflows a `long` FIRST and only then runs out of
            // calendar, and the two have different messages -- `plusDays` is
            // `Math.addExact(epochDay, daysToAdd)` before
            // `LocalDate.ofEpochDay`'s range check. Reproduce that order, or
            // `dateAdd(Long.MAX_VALUE, 'DAYS')` reports egret's narrower range
            // for what is really Java's own arithmetic overflowing.
            ChronoUnit::Days => {
                epoch_day(local).checked_add(amount).ok_or_else(overflow)?;
                plus_days(local, amount).ok_or_else(|| range(amount / 366))
            }
            ChronoUnit::Weeks => {
                let days = amount.checked_mul(7).ok_or_else(overflow)?;
                epoch_day(local).checked_add(days).ok_or_else(overflow)?;
                plus_days(local, days).ok_or_else(|| range(days / 366))
            }
            ChronoUnit::Months => {
                month_count(local)
                    .checked_add(amount)
                    .ok_or_else(overflow)?;
                plus_months(local, amount).ok_or_else(|| range(amount / 12))
            }
            ChronoUnit::Years
            | ChronoUnit::Decades
            | ChronoUnit::Centuries
            | ChronoUnit::Millennia => {
                let scale: i64 = match self {
                    ChronoUnit::Years => 1,
                    ChronoUnit::Decades => 10,
                    ChronoUnit::Centuries => 100,
                    _ => 1_000,
                };
                let years = amount.checked_mul(scale).ok_or_else(overflow)?;
                let target = i64::from(local.year())
                    .checked_add(years)
                    .ok_or_else(overflow)?;
                if !(-999_999_999..=999_999_999).contains(&target) {
                    return Err(out_of_range(target));
                }
                let months = years.checked_mul(12).ok_or_else(overflow)?;
                plus_months(local, months).ok_or_else(|| range(target))
            }
            _ => unreachable!("only date-based units reach add_to_local"),
        }
    }
}

/// **egret's date range is `chrono`'s, and it is narrower than `java.time`'s.**
///
/// A `NaiveDateTime` spans roughly +/-262143 years; `LocalDateTime` spans
/// +/-999999999. So `dateAdd(100000000, 'DAYS')` is `+275814-09-28` in Kestra
/// and is not representable here.
///
/// The limit is `chrono`'s, not a choice -- but the *message* is a choice, and
/// a first cut of this reported `long overflow`, which is Java's sentence for
/// genuine `long` arithmetic overflowing and is a different cause entirely.
/// The unit test that covered it asserted exactly the two extremes
/// (`i64::MAX` DAYS, `1e9` YEARS) where the two engines happen to agree, while
/// the whole middle diverged silently. Both found by the fix round's review.
///
/// `filter-dateadd-past-egrets-date-range` is the baselined corpus entry.
fn out_of_chrono_range(years: i64) -> String {
    format!(
        "the 'dateAdd' filter cannot represent a date about {years} years away: egret's dates are \
         chrono's, which reach about +/-262143, narrower than java.time's +/-999999999"
    )
}

/// `LocalDate.toEpochDay()`, the value `plusDays` adds to with `addExact`.
fn epoch_day(local: NaiveDateTime) -> i64 {
    local
        .date()
        .signed_duration_since(NaiveDate::from_ymd_opt(1970, 1, 1).expect("a real date"))
        .num_days()
}

/// `LocalDate`'s `prolepticMonth`, the value `plusMonths` adds to with
/// `addExact`.
fn month_count(local: NaiveDateTime) -> i64 {
    i64::from(local.year()) * 12 + i64::from(local.month0())
}

/// A whole number of days plus a sub-day remainder in nanoseconds, as a
/// `TimeDelta` — or [`out_of_chrono_range`] when the span itself is more than
/// a `TimeDelta` can hold. That is egret's limit and it says so; it is not
/// `long overflow`, which is Java's arithmetic failing.
fn delta_from(days: i64, nanos: i64) -> Result<chrono::TimeDelta, String> {
    let range = || out_of_chrono_range(days / 366);
    chrono::TimeDelta::try_days(days)
        .and_then(|d| d.checked_add(&chrono::TimeDelta::nanoseconds(nanos)))
        .ok_or_else(range)
}

fn plus_days(local: NaiveDateTime, amount: i64) -> Option<NaiveDateTime> {
    if amount >= 0 {
        local.checked_add_days(Days::new(u64::try_from(amount).ok()?))
    } else {
        local.checked_sub_days(Days::new(u64::try_from(amount.checked_neg()?).ok()?))
    }
}

fn plus_months(local: NaiveDateTime, amount: i64) -> Option<NaiveDateTime> {
    if amount >= 0 {
        local.checked_add_months(Months::new(u32::try_from(amount).ok()?))
    } else {
        local.checked_sub_months(Months::new(u32::try_from(amount.checked_neg()?).ok()?))
    }
}

// ---------------------------------------------------------------------------
// The pattern
// ---------------------------------------------------------------------------

/// One piece of a parsed Java pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Piece {
    /// Literal text, with the quoting already resolved.
    Literal(String),
    /// A run of `count` copies of `letter`.
    Field {
        letter: char,
        count: usize,
    },
    /// `p`, `pp`, ... — pad whatever comes next to this width with spaces.
    Pad(usize),
    /// `[` and `]`. An optional section only prints when every field inside it
    /// is available; a `ZonedDateTime` has all of them, so for **formatting**
    /// these are transparent. They still have to be recognised, because `]`
    /// without a `[` is an error and `[`/`]` are not literal characters.
    OptionalStart,
    OptionalEnd,
}

/// A parsed `DateTimeFormatter.ofPattern` pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Pattern {
    pieces: Vec<Piece>,
}

/// The pattern letters `DateTimeFormatterBuilder.parsePattern` accepts, with
/// the count range each allows. A letter outside this table is
/// `Unknown pattern letter`; a count outside its range is one of the three
/// messages below, all read off the JDK.
fn letter_limits(letter: char) -> Option<(usize, usize)> {
    Some(match letter {
        'G' => (1, 5),
        'y' | 'u' => (1, 19),
        'M' | 'L' => (1, 5),
        'd' => (1, 2),
        'D' => (1, 3),
        'E' => (1, 5),
        'e' => (1, 5),
        // `cc` is rejected by the JDK ("Invalid pattern") while `c`, `ccc`,
        // `cccc` and `ccccc` are fine; handled at the call site.
        'c' => (1, 5),
        'w' => (1, 2),
        // The week-BASED year, which is not `y`: for a date in the days that
        // a locale's week rules push into the neighbouring year, `Y` and `y`
        // disagree. 2023-12-31 is a Sunday, so under `en`'s Sunday-first
        // rules it is week 1 of `Y` = 2024 while `y` is still 2023.
        //
        // **And it is the one letter with NO upper bound.** `y` and `u` go
        // through `appendValue(field, count, 19, …)` and stop at 19;
        // `WeekBasedFieldPrinterParser` has no such cap, so `Y` × 20 is
        // `00000000000000002024` in Kestra and was a refusal here. The output
        // is still linear in the pattern's own length, so the bound argument
        // in `runners::filters` survives -- but it survives for a different
        // reason than it claimed, and the claim is corrected there.
        'Y' => (1, usize::MAX),
        'W' | 'F' => (1, 1),
        'Q' | 'q' => (1, 5),
        'a' => (1, 1),
        'h' | 'H' | 'k' | 'K' | 'm' | 's' => (1, 2),
        'S' => (1, 9),
        'A' | 'N' | 'n' | 'g' => (1, 19),
        'V' => (2, 2),
        'z' => (1, 4),
        'O' => (1, 4),
        'X' | 'x' | 'Z' => (1, 5),
        // `v` and `B` are the two letters whose ACCEPTED COUNTS are not a
        // range: `v` takes 1 or 4, `B` takes 1, 4 or 5, and every other count
        // is `Wrong number of pattern letters` -- a fourth JDK sentence,
        // distinct from `Too many pattern letters`. Enforced at the call site.
        //
        // A first cut of this table had no `'v'` arm at all, so `date('vvvv')`
        // said `Unknown pattern letter: v` where Java says the pattern is
        // valid and prints `Japan Time`. That is worse than refusing it: it
        // tells an author their pattern is wrong when it is not. Found by the
        // fix round's probe sweep, which walked every ASCII letter at counts
        // 1-11 rather than the letters someone thought to list.
        'v' => (1, 4),
        'B' => (1, 5),
        _ => return None,
    })
}

impl Pattern {
    /// `DateTimeFormatter.ofPattern(pattern)`.
    ///
    /// Reproduces the JDK's own `IllegalArgumentException` text for each way a
    /// pattern can be rejected, because those are what Kestra surfaces to a
    /// flow author:
    ///
    /// | pattern | message |
    /// |---|---|
    /// | `ddd` | `Too many pattern letters: d` |
    /// | `yyyy'MM` | `Pattern ends with an incomplete string literal: yyyy'MM` |
    /// | `yyyy#MM` | `Pattern includes reserved character: '#'` |
    /// | `yyyy]` | `Pattern invalid as it contains ] without previous [` |
    /// | `SSSSSSSSSS` | `Minimum width must be from 0 to 9 inclusive but was 10` |
    /// | `y`×20 | `The minimum width must be from 1 to 19 inclusive but was 20` |
    /// | `VVV` | `Pattern letter count must be 2: V` |
    /// | `OO` | `Pattern letter count must be 1 or 4: O` |
    ///
    /// All eight measured through the oracle harness, not guessed.
    pub(super) fn parse(pattern: &str) -> Result<Self, String> {
        let chars: Vec<char> = pattern.chars().collect();
        let mut pieces = Vec::new();
        let mut depth = 0usize;
        let mut i = 0usize;
        while i < chars.len() {
            let c = chars[i];
            if c.is_ascii_alphabetic() {
                let mut count = 1;
                while i + count < chars.len() && chars[i + count] == c {
                    count += 1;
                }
                i += count;
                if c == 'p' {
                    // `padNext` needs a **pattern letter** to pad, not merely
                    // something. `pp'x'`, `p[H]` and `p-` are all rejected by
                    // the JDK, and a first cut of this only checked that the
                    // pattern did not END there -- so those three rendered
                    // here and threw in Kestra. The JDK quotes the rest of the
                    // pattern from the `p` run back in the message.
                    if !chars.get(i).is_some_and(|c| c.is_ascii_alphabetic()) {
                        let rest: String = chars[i - count..].iter().collect();
                        return Err(format!(
                            "Pad letter 'p' must be followed by valid pad pattern: {rest}"
                        ));
                    }
                    pieces.push(Piece::Pad(count));
                    continue;
                }
                let Some((min, max)) = letter_limits(c) else {
                    return Err(format!("Unknown pattern letter: {c}"));
                };
                if c == 'V' && count != 2 {
                    return Err("Pattern letter count must be 2: V".to_string());
                }
                if c == 'O' && count != 1 && count != 4 {
                    return Err("Pattern letter count must be 1 or 4: O".to_string());
                }
                if c == 'c' && count == 2 {
                    return Err(format!("Invalid pattern \"{pattern}\""));
                }
                // The two letters with a non-contiguous count set.
                if (c == 'v' && !matches!(count, 1 | 4))
                    || (c == 'B' && !matches!(count, 1 | 4 | 5))
                {
                    return Err(format!("Wrong number of pattern letters: {c}"));
                }
                if count > max {
                    return Err(match c {
                        'S' => {
                            format!("Minimum width must be from 0 to 9 inclusive but was {count}")
                        }
                        'y' | 'u' | 'Y' | 'A' | 'N' | 'n' | 'g' => format!(
                            "The minimum width must be from 1 to 19 inclusive but was {count}"
                        ),
                        _ => format!("Too many pattern letters: {c}"),
                    });
                }
                debug_assert!(count >= min);
                pieces.push(Piece::Field { letter: c, count });
                continue;
            }
            match c {
                '\'' => {
                    let mut text = String::new();
                    let mut j = i + 1;
                    loop {
                        let Some(&ch) = chars.get(j) else {
                            return Err(format!(
                                "Pattern ends with an incomplete string literal: {pattern}"
                            ));
                        };
                        if ch == '\'' {
                            // `''` inside a literal is one quote; `''` on its
                            // own (an empty literal) is also one quote, which
                            // is why this is checked before the close.
                            if chars.get(j + 1) == Some(&'\'') {
                                text.push('\'');
                                j += 2;
                                continue;
                            }
                            break;
                        }
                        text.push(ch);
                        j += 1;
                    }
                    if text.is_empty() && j == i + 1 {
                        // `''` -- a literal quote, not an empty literal.
                        text.push('\'');
                    }
                    pieces.push(Piece::Literal(text));
                    i = j + 1;
                }
                '[' => {
                    depth += 1;
                    pieces.push(Piece::OptionalStart);
                    i += 1;
                }
                ']' => {
                    if depth == 0 {
                        return Err(
                            "Pattern invalid as it contains ] without previous [".to_string()
                        );
                    }
                    depth -= 1;
                    pieces.push(Piece::OptionalEnd);
                    i += 1;
                }
                '#' | '{' | '}' => {
                    return Err(format!("Pattern includes reserved character: '{c}'"));
                }
                _ => {
                    pieces.push(Piece::Literal(c.to_string()));
                    i += 1;
                }
            }
        }
        Ok(Pattern { pieces })
    }

    /// Writes `zdt` through this pattern.
    ///
    /// `zdt` must already have been moved into `zone`
    /// ([`Zdt::with_zone`]) -- Java's `DateTimeFormatter.withZone` does that
    /// before formatting, which is why `{{ d | date('VV', timeZone='UTC') }}`
    /// on an input carrying `[America/New_York]` prints `UTC` and not the
    /// input's own zone. Measured.
    fn format(&self, zdt: &Zdt, _locale: Locale) -> Result<String, String> {
        let mut out = String::new();
        let mut pad: Option<usize> = None;
        for piece in &self.pieces {
            match piece {
                Piece::OptionalStart | Piece::OptionalEnd => {}
                Piece::Pad(width) => {
                    pad = Some(*width);
                    continue;
                }
                Piece::Literal(text) => {
                    push_padded(&mut out, text, pad.take())?;
                }
                Piece::Field { letter, count } => {
                    let text = format_field(*letter, *count, zdt)?;
                    push_padded(&mut out, &text, pad.take())?;
                }
            }
        }
        Ok(out)
    }
}

/// `PadPrinterParserDecorator.format` — left-pads with spaces, and **throws
/// when the value is wider than the pad** rather than letting it through.
///
/// `date('pH')` at 05:00 is ` 5` and at 10:00 is a `DateTimeException`, which
/// is why `filter-date-pattern-pad`'s 05:07 input was the one clock time where
/// the entry could not see the difference. Found by the fix round's review.
fn push_padded(out: &mut String, text: &str, pad: Option<usize>) -> Result<(), String> {
    let Some(width) = pad else {
        out.push_str(text);
        return Ok(());
    };
    let len = text.chars().count();
    if len > width {
        return Err(format!(
            "Cannot print as output of {len} characters exceeds pad width of {width}"
        ));
    }
    for _ in len..width {
        out.push(' ');
    }
    out.push_str(text);
    Ok(())
}

const MONTHS_FULL: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const MONTHS_SHORT: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const MONTHS_NARROW: [&str; 12] = ["J", "F", "M", "A", "M", "J", "J", "A", "S", "O", "N", "D"];
/// Indexed by `Weekday::num_days_from_monday()`.
const DAYS_FULL: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const DAYS_SHORT: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const DAYS_NARROW: [&str; 7] = ["M", "T", "W", "T", "F", "S", "S"];
const QUARTERS_FULL: [&str; 4] = ["1st quarter", "2nd quarter", "3rd quarter", "4th quarter"];

/// `WeekFields.of(Locale("en"))` — **Sunday-first, one minimal day**.
///
/// Measured against the JDK rather than assumed: `WeekFields.of` reads the
/// locale's *country*, so a language-only `en` falls back to Sunday/1 while
/// `en_GB` is Monday/4 and the harness's own default `en_VN` is Monday/1.
/// [`Locale`] refuses everything but `en` and `en_US` for exactly this reason.
const FIRST_DAY_OF_WEEK: u32 = 7; // Sunday, as `num_days_from_monday() + 1`
const MINIMAL_DAYS_IN_FIRST_WEEK: i32 = 1;

fn format_field(letter: char, count: usize, zdt: &Zdt) -> Result<String, String> {
    let local = zdt.local;
    let date = local.date();
    let nano = local.nanosecond().min(999_999_999);
    Ok(match letter {
        'G' => era_text(date.year(), count).to_string(),
        // `y` is the year OF ERA (always positive); `u` is the proleptic year
        // (negative before year 1). `-0044` prints `45` under `y` and `-44`
        // under `u` -- the one place the two letters visibly disagree.
        'y' => year_field(i64::from(year_of_era(date.year())), count),
        'u' => year_field(i64::from(date.year()), count),
        'M' | 'L' => month_text(date.month0() as usize, count),
        'd' => zero_pad(i64::from(date.day()), count),
        'D' => zero_pad(i64::from(date.ordinal()), count),
        'E' => day_text(date.weekday().num_days_from_monday() as usize, count.max(3)),
        'e' | 'c' if count <= 2 => zero_pad(i64::from(localized_day_of_week(&date)), count),
        'e' | 'c' => day_text(date.weekday().num_days_from_monday() as usize, count),
        'w' => zero_pad(i64::from(week_of_week_based_year(&date)), count),
        'Y' => year_field(i64::from(week_based_year(&date)), count),
        'W' => zero_pad(i64::from(week_of_month(&date)), count),
        'F' => zero_pad(i64::from((date.day() - 1) / 7 + 1), count),
        'Q' | 'q' => quarter_text((date.month0() / 3) as usize, count),
        'a' => (if local.hour() < 12 { "AM" } else { "PM" }).to_string(),
        'h' => zero_pad(i64::from(clock_hour(local.hour() % 12, 12)), count),
        'H' => zero_pad(i64::from(local.hour()), count),
        'k' => zero_pad(i64::from(clock_hour(local.hour(), 24)), count),
        'K' => zero_pad(i64::from(local.hour() % 12), count),
        'm' => zero_pad(i64::from(local.minute()), count),
        's' => zero_pad(i64::from(local.second()), count),
        // `S` TRUNCATES rather than rounding: `.987654321` is `9`, `98`, `987`.
        'S' => format!("{nano:09}")[..count].to_string(),
        'A' => zero_pad(milli_of_day(&local), count),
        'N' => zero_pad(nano_of_day(&local), count),
        'n' => zero_pad(i64::from(nano), count),
        // Modified Julian Day: the epoch day plus 40587.
        'g' => zero_pad(
            date.and_time(NaiveTime::MIN).and_utc().timestamp() / 86_400 + 40_587,
            count,
        ),
        'V' => zdt.zone.id(),
        'X' => offset_text(zdt.offset, count, true),
        'x' => offset_text(zdt.offset, count, false),
        'Z' => match count {
            1..=3 => offset_text(zdt.offset, 2, false),
            4 => localized_gmt(zdt.offset, 4),
            _ => offset_text(zdt.offset, 5, true),
        },
        'O' => localized_gmt(zdt.offset, count),
        // The three letters whose output is text out of a locale database.
        // `v`/`vvvv` are the *generic* zone name (`JT` / `Japan Time`), a
        // third CLDR table beside `z`'s specific names and `B`'s day periods.
        // All three are refused HERE rather than at parse time, so the pattern
        // is still reported valid-or-not the way Java reports it.
        'z' | 'B' | 'v' => {
            return Err(format!(
                "the 'date' filters cannot render pattern letter '{letter}': it needs the locale's \
                 zone/day-period names, which egret does not carry"
            ))
        }
        other => return Err(format!("Unknown pattern letter: {other}")),
    })
}

fn era_text(year: i32, count: usize) -> &'static str {
    match (year > 0, count) {
        (true, 1..=3) => "AD",
        (true, 4) => "Anno Domini",
        (true, _) => "A",
        (false, 1..=3) => "BC",
        (false, 4) => "Before Christ",
        (false, _) => "B",
    }
}

fn year_of_era(year: i32) -> i32 {
    if year > 0 {
        year
    } else {
        1 - year
    }
}

fn month_text(month0: usize, count: usize) -> String {
    match count {
        1 | 2 => zero_pad(month0 as i64 + 1, count),
        3 => MONTHS_SHORT[month0].to_string(),
        4 => MONTHS_FULL[month0].to_string(),
        _ => MONTHS_NARROW[month0].to_string(),
    }
}

fn day_text(day0: usize, count: usize) -> String {
    match count {
        1..=3 => DAYS_SHORT[day0].to_string(),
        4 => DAYS_FULL[day0].to_string(),
        _ => DAYS_NARROW[day0].to_string(),
    }
}

fn quarter_text(quarter0: usize, count: usize) -> String {
    match count {
        1 | 2 => zero_pad(quarter0 as i64 + 1, count),
        3 => format!("Q{}", quarter0 + 1),
        4 => QUARTERS_FULL[quarter0].to_string(),
        _ => (quarter0 + 1).to_string(),
    }
}

/// `hour == 0` prints as the wrap value: `h` is 1-12 and `k` is 1-24.
fn clock_hour(hour: u32, wrap: u32) -> u32 {
    if hour == 0 {
        wrap
    } else {
        hour
    }
}

fn milli_of_day(local: &NaiveDateTime) -> i64 {
    i64::from(local.num_seconds_from_midnight()) * 1_000 + i64::from(local.nanosecond() / 1_000_000)
}

fn nano_of_day(local: &NaiveDateTime) -> i64 {
    i64::from(local.num_seconds_from_midnight()) * 1_000_000_000
        + i64::from(local.nanosecond().min(999_999_999))
}

/// `WeekFields.dayOfWeek()` — 1 for the locale's first day of the week.
fn localized_day_of_week(date: &NaiveDate) -> u32 {
    let iso = date.weekday().num_days_from_monday() + 1; // Monday = 1
    (iso + 7 - FIRST_DAY_OF_WEEK) % 7 + 1
}

/// `WeekFields.startOfWeekOffset`, copied from the JDK.
fn start_of_week_offset(day: i32, dow: i32) -> i32 {
    let week_start = (day - dow).rem_euclid(7);
    if week_start + 1 > MINIMAL_DAYS_IN_FIRST_WEEK {
        7 - week_start
    } else {
        -week_start
    }
}

fn compute_week(offset: i32, day: i32) -> i32 {
    (7 + offset + (day - 1)) / 7
}

/// `WeekFields.weekOfMonth()`.
fn week_of_month(date: &NaiveDate) -> i32 {
    let dow = localized_day_of_week(date) as i32;
    let dom = date.day() as i32;
    compute_week(start_of_week_offset(dom, dow), dom)
}

/// `WeekFields.weekOfWeekBasedYear()`, including both of the JDK's rollover
/// branches: a week 0 belongs to the previous year, and a week past 50 may be
/// week 1 of the next.
fn week_of_week_based_year(date: &NaiveDate) -> i32 {
    let dow = localized_day_of_week(date) as i32;
    let doy = date.ordinal() as i32;
    let offset = start_of_week_offset(doy, dow);
    let week = compute_week(offset, doy);
    if week == 0 {
        let last_of_previous = *date - Days::new(doy as u64);
        return week_of_week_based_year(&last_of_previous);
    }
    if week > 50 {
        let year_len = if date.leap_year() { 366 } else { 365 };
        let new_year_week = compute_week(offset, year_len + MINIMAL_DAYS_IN_FIRST_WEEK);
        if week >= new_year_week {
            return week - new_year_week + 1;
        }
    }
    week
}

/// `WeekFields.weekBasedYear()` — the year `w` counts within, which is not
/// the calendar year at either end of it.
fn week_based_year(date: &NaiveDate) -> i32 {
    let dow = localized_day_of_week(date) as i32;
    let doy = date.ordinal() as i32;
    let offset = start_of_week_offset(doy, dow);
    let week = compute_week(offset, doy);
    if week == 0 {
        return date.year() - 1;
    }
    if week > 50 {
        let year_len = if date.leap_year() { 366 } else { 365 };
        if week >= compute_week(offset, year_len + MINIMAL_DAYS_IN_FIRST_WEEK) {
            return date.year() + 1;
        }
    }
    date.year()
}

/// `X` (`zero_is_z`) and `x` — the ISO-8601 offset forms.
///
/// Count 1 prints the hour alone unless the minutes are non-zero, in which
/// case it prints `+HHmm` with no separator. Measured on `Asia/Kathmandu`
/// (`+05:45`), whose `X` is `+0545`.
fn offset_text(offset: FixedOffset, count: usize, zero_is_z: bool) -> String {
    let total = offset.local_minus_utc();
    if total == 0 && zero_is_z {
        return "Z".to_string();
    }
    let sign = if total < 0 { '-' } else { '+' };
    let abs = total.abs();
    let (h, m, s) = (abs / 3600, (abs % 3600) / 60, abs % 60);
    let mut out = String::new();
    let _ = write!(out, "{sign}{h:02}");
    match count {
        1 => {
            if m != 0 || s != 0 {
                let _ = write!(out, "{m:02}");
            }
        }
        2 => {
            let _ = write!(out, "{m:02}");
        }
        3 => {
            let _ = write!(out, ":{m:02}");
        }
        4 => {
            let _ = write!(out, "{m:02}");
            if s != 0 {
                let _ = write!(out, "{s:02}");
            }
        }
        _ => {
            let _ = write!(out, ":{m:02}");
            if s != 0 {
                let _ = write!(out, ":{s:02}");
            }
        }
    }
    out
}

/// `O`/`OOOO` and `ZZZZ` — the localized GMT offset. `GMT` alone at zero;
/// `GMT+9` short and `GMT+09:00` long. The word `GMT` is the English form,
/// which is the only one [`Locale`] admits.
fn localized_gmt(offset: FixedOffset, count: usize) -> String {
    let total = offset.local_minus_utc();
    if total == 0 {
        return "GMT".to_string();
    }
    let sign = if total < 0 { '-' } else { '+' };
    let abs = total.abs();
    let (h, m, s) = (abs / 3600, (abs % 3600) / 60, abs % 60);
    if count == 1 {
        let mut out = format!("GMT{sign}{h}");
        if m != 0 || s != 0 {
            let _ = write!(out, ":{m:02}");
            if s != 0 {
                let _ = write!(out, ":{s:02}");
            }
        }
        out
    } else {
        let mut out = format!("GMT{sign}{h:02}:{m:02}");
        if s != 0 {
            let _ = write!(out, ":{s:02}");
        }
        out
    }
}

fn zero_pad(value: i64, width: usize) -> String {
    if value < 0 {
        format!("-{:0>width$}", value.unsigned_abs(), width = width)
    } else {
        format!("{value:0>width$}")
    }
}

/// `y`, `u` and `Y` — the three year letters, which take **three different
/// sign styles depending on the letter count**.
///
/// `DateTimeFormatterBuilder.parseField` reads:
///
/// ```text
/// count == 2  -> appendValueReduced(field, 2, 2, BASE_DATE)
/// count <  4  -> appendValue(field, count, 19, SignStyle.NORMAL)
/// else        -> appendValue(field, count, 19, SignStyle.EXCEEDS_PAD)
/// ```
///
/// Both edges were wrong in the first cut, and both are silent — a wrong year,
/// not an error:
///
/// - **`ReducedPrinterParser.getValue` is `Math.abs(value) % 100`**, not a
///   modulo of the signed value. `uu` on year -44 is `44` and this wrote `56`;
///   on year -1 it is `01` and this wrote `99`. `yy` was right only because
///   year-of-era is always positive — which is exactly why the original
///   `filter-date-pattern-era-bc` entry, which probes `y|u|yyyy|uuuu` on a BC
///   date, could not see it: it omits the one count that breaks.
/// - **`SignStyle.EXCEEDS_PAD` writes a leading `+`** when the value needs
///   more digits than the pad width. `yyyy` on year 10000 is `+10000`, so
///   Kestra's DEFAULT `iso` format is `+10000-01-15T…`. Note it applies at
///   count >= 4 only, and only when the digits *exceed* the width: `yyyyy` on
///   year 10000 is `10000`, with no sign, and `yyyyyy` is `010000`.
///
/// Both found by the fix round's review, and both reproduce in the probe
/// sweep across twelve year magnitudes.
fn year_field(value: i64, count: usize) -> String {
    if count == 2 {
        // `Math.abs(value) % 100` -- absolute value FIRST.
        return format!("{:02}", value.unsigned_abs() % 100);
    }
    if count < 4 {
        // SignStyle.NORMAL: a sign only when negative.
        return zero_pad(value, count);
    }
    // SignStyle.EXCEEDS_PAD.
    let digits = value.unsigned_abs().to_string().len();
    if value >= 0 && digits > count {
        format!("+{value}")
    } else {
        zero_pad(value, count)
    }
}

// ---------------------------------------------------------------------------
// Named formats
// ---------------------------------------------------------------------------

/// `AbstractDate.formatter(format)` — the STYLES map, then the FORMATTERS map,
/// then `DateTimeFormatter.ofPattern`.
///
/// Both maps are read off `AbstractDate`'s static initialiser in the
/// `io.kestra:core:2.0.0-rc13` artifact (`javap -p -c`), not off the checkout.
/// Note the lookup order: a name in STYLES wins over the same name in
/// FORMATTERS, and **an unknown name is not an error** — it falls through to
/// `ofPattern`, which is why `date('nope_format')` fails with
/// `Unknown pattern letter: o`.
#[derive(Clone, Debug)]
pub(super) struct Format {
    name: String,
    kind: FormatKind,
}

#[derive(Clone, Debug)]
enum FormatKind {
    /// The six FORMATTERS entries Kestra builds with `ofPattern`, the four
    /// `FormatStyle`s (whose CLDR patterns are inlined, see [`Format::resolve`]),
    /// and every user-written pattern.
    Pattern(Pattern),
    /// The twelve FORMATTERS entries that are `java.time` constants built by
    /// `DateTimeFormatterBuilder`, which have no pattern spelling.
    BuiltIn(BuiltIn),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BuiltIn {
    IsoDateTime,
    IsoDate,
    IsoTime,
    IsoLocalDate,
    IsoInstant,
    IsoLocalDateTime,
    IsoLocalTime,
    IsoOffsetTime,
    IsoOrdinalDate,
    IsoWeekDate,
    IsoZonedDateTime,
    Rfc1123DateTime,
}

impl Format {
    pub(super) fn resolve(name: &str) -> Result<Self, String> {
        // The four `FormatStyle`s. Kestra calls
        // `DateTimeFormatter.ofLocalizedDateTime(style)`, whose pattern comes
        // from CLDR; the English patterns are inlined here, measured with
        // `DateTimeFormatterBuilder.getLocalizedDateTimePattern` on the same
        // JDK the goldens are generated with (Corretto 25 / CLDR). The narrow
        // no-break space before `a` is CLDR's, not a typo.
        //
        // `full` and `long` end in `zzzz`/`z`, so they are accepted here and
        // refused by [`format_field`] with the reason -- rather than being
        // special-cased into a different error that hides which letter is the
        // problem.
        let style = match name {
            "full" => Some("EEEE, MMMM d, y, h:mm:ss\u{202f}a zzzz"),
            "long" => Some("MMMM d, y, h:mm:ss\u{202f}a z"),
            "medium" => Some("MMM d, y, h:mm:ss\u{202f}a"),
            "short" => Some("M/d/yy, h:mm\u{202f}a"),
            _ => None,
        };
        if let Some(pattern) = style {
            return Ok(Format {
                name: name.to_string(),
                kind: FormatKind::Pattern(Pattern::parse(pattern)?),
            });
        }

        let pattern = match name {
            "iso" => Some("yyyy-MM-dd'T'HH:mm:ss.SSSSSSXXX"),
            "iso_milli" => Some("yyyy-MM-dd'T'HH:mm:ss.SSSXXX"),
            "iso_sec" => Some("yyyy-MM-dd'T'HH:mm:ssXXX"),
            "sql" => Some("yyyy-MM-dd HH:mm:ss.SSSSSS"),
            "sql_milli" => Some("yyyy-MM-dd HH:mm:ss.SSS"),
            "sql_sec" => Some("yyyy-MM-dd HH:mm:ss"),
            _ => None,
        };
        if let Some(pattern) = pattern {
            return Ok(Format {
                name: name.to_string(),
                kind: FormatKind::Pattern(Pattern::parse(pattern)?),
            });
        }

        let built_in = match name {
            "iso_date_time" => Some(BuiltIn::IsoDateTime),
            "iso_date" => Some(BuiltIn::IsoDate),
            "iso_time" => Some(BuiltIn::IsoTime),
            "iso_local_date" => Some(BuiltIn::IsoLocalDate),
            "iso_instant" => Some(BuiltIn::IsoInstant),
            "iso_local_date_time" => Some(BuiltIn::IsoLocalDateTime),
            "iso_local_time" => Some(BuiltIn::IsoLocalTime),
            "iso_offset_time" => Some(BuiltIn::IsoOffsetTime),
            "iso_ordinal_date" => Some(BuiltIn::IsoOrdinalDate),
            "iso_week_date" => Some(BuiltIn::IsoWeekDate),
            "iso_zoned_date_time" => Some(BuiltIn::IsoZonedDateTime),
            "rfc_1123_date_time" => Some(BuiltIn::Rfc1123DateTime),
            _ => None,
        };
        if let Some(built_in) = built_in {
            return Ok(Format {
                name: name.to_string(),
                kind: FormatKind::BuiltIn(built_in),
            });
        }

        Ok(Format {
            name: name.to_string(),
            kind: FormatKind::Pattern(Pattern::parse(name)?),
        })
    }

    /// `formatter.withLocale(locale).withZone(zone).format(zdt)`.
    pub(super) fn format(&self, zdt: &Zdt, zone: &Zone, locale: Locale) -> Result<String, String> {
        let shown = zdt.with_zone(zone);
        match &self.kind {
            FormatKind::Pattern(pattern) => pattern.format(&shown, locale),
            // ISO_INSTANT formats INSTANT_SECONDS, so `withZone` cannot move
            // it: it is always UTC. Measured -- `iso_instant` answers the same
            // string for `timeZone='UTC'` and `timeZone='Asia/Tokyo'`.
            FormatKind::BuiltIn(BuiltIn::IsoInstant) => Ok(format_iso_instant(&shown)),
            FormatKind::BuiltIn(built_in) => format_built_in(*built_in, &shown),
        }
    }

    /// The `existingFormat` path: `ZonedDateTime.parse(text, formatter)` with
    /// Kestra's two fallbacks.
    ///
    /// # The `locale` argument does NOT reach this side
    ///
    /// `AbstractDate.convert` calls `formatter(existingFormat)` with **no**
    /// `.withLocale(…)`, while `AbstractDate.format` calls
    /// `formatter(format).withLocale(locale).withZone(zone)`. Read off the
    /// bytecode. So Kestra parses an `existingFormat` under
    /// `Locale.getDefault(FORMAT)` — `en_VN` on the machine that generates
    /// `golden.json` — whatever the template's `locale` argument says: an
    /// `existingFormat` of `MMM d uuuu` with `locale='fr'` still wants the
    /// English `Jan`.
    ///
    /// egret's parser is locale-INDEPENDENT (English names, and the week
    /// fields consumed without meaning), which matches the pinned harness and
    /// is a narrower behaviour rather than a different one. It is also why no
    /// corpus entry may put `e`, `c`, `w`, `W` or `Y` inside an
    /// `existingFormat`: their meaning there is the harness machine's region.
    ///
    /// **The twelve `java.time` constants are refused as an `existingFormat`.**
    /// They are `DateTimeFormatterBuilder` compositions with no pattern
    /// spelling, and four of them (`iso_instant`, `iso_time`,
    /// `iso_local_time`, `iso_offset_time`) fail in Kestra too, because they
    /// yield no date. The other eight are a divergence, stated here and in
    /// `oracle/README.md` rather than approximated.
    pub(super) fn parse_text(&self, text: &str, zone: &Zone) -> Result<Zdt, String> {
        match &self.kind {
            FormatKind::Pattern(pattern) => pattern.parse_text(text, zone),
            FormatKind::BuiltIn(_) => Err(format!(
                "the 'date' filters cannot use '{}' as an 'existingFormat': it is a java.time \
                 constant with no pattern spelling",
                self.name
            )),
        }
    }
}

/// The variable-width fraction the ISO_* formatters print:
/// `appendFraction(NANO_OF_SECOND, 0, 9, true)` — nothing at all when the
/// nanosecond is zero, and trailing zeros trimmed otherwise, so `.120` prints
/// as `.12`.
fn iso_fraction(nano: u32) -> String {
    if nano == 0 {
        return String::new();
    }
    let digits = format!("{nano:09}");
    format!(".{}", digits.trim_end_matches('0'))
}

/// `DateTimeFormatterBuilder.appendInstant()` — groups of **three** digits,
/// which is why `iso_instant` prints `.120` where `iso_local_time` prints
/// `.12` for the same value.
fn instant_fraction(nano: u32) -> String {
    if nano == 0 {
        return String::new();
    }
    let digits = format!("{nano:09}");
    let keep = if nano.is_multiple_of(1_000_000) {
        3
    } else if nano.is_multiple_of(1_000) {
        6
    } else {
        9
    };
    format!(".{}", &digits[..keep])
}

/// `appendValue(YEAR, 4, 10, EXCEEDS_PAD)`.
fn iso_year(year: i32) -> String {
    if year < 0 {
        format!("-{:04}", year.unsigned_abs())
    } else if year > 9999 {
        format!("+{year}")
    } else {
        format!("{year:04}")
    }
}

fn iso_local_date(zdt: &Zdt) -> String {
    let d = zdt.local.date();
    format!("{}-{:02}-{:02}", iso_year(d.year()), d.month(), d.day())
}

fn iso_local_time(zdt: &Zdt) -> String {
    let t = zdt.local.time();
    format!(
        "{:02}:{:02}:{:02}{}",
        t.hour(),
        t.minute(),
        t.second(),
        iso_fraction(zdt.nano())
    )
}

/// `['['VV']']` — appended only when the zone is a region. A bare
/// `ZoneOffset` (`Z`, `+05:30`) has none, which is what separates
/// `2024-01-15T10:30:00Z[UTC]` from `2024-01-15T15:30:00+05:30`.
fn zone_suffix(zdt: &Zdt) -> String {
    match &zdt.zone {
        Zone::Region(_) => format!("[{}]", zdt.zone.id()),
        Zone::Fixed(_, id) if id == "Z" || id.starts_with('+') || id.starts_with('-') => {
            String::new()
        }
        Zone::Fixed(..) => format!("[{}]", zdt.zone.id()),
    }
}

fn format_iso_instant(zdt: &Zdt) -> String {
    let utc = zdt.with_zone(&Zone::Fixed(
        FixedOffset::east_opt(0).expect("0 is a valid offset"),
        "Z".to_string(),
    ));
    let d = utc.local.date();
    let t = utc.local.time();
    format!(
        "{}-{:02}-{:02}T{:02}:{:02}:{:02}{}Z",
        iso_year(d.year()),
        d.month(),
        d.day(),
        t.hour(),
        t.minute(),
        t.second(),
        instant_fraction(utc.nano())
    )
}

fn format_built_in(built_in: BuiltIn, zdt: &Zdt) -> Result<String, String> {
    let offset = offset_id(zdt.offset);
    Ok(match built_in {
        BuiltIn::IsoLocalDate => iso_local_date(zdt),
        BuiltIn::IsoDate => format!("{}{offset}", iso_local_date(zdt)),
        BuiltIn::IsoLocalTime => iso_local_time(zdt),
        BuiltIn::IsoTime | BuiltIn::IsoOffsetTime => format!("{}{offset}", iso_local_time(zdt)),
        BuiltIn::IsoLocalDateTime => format!("{}T{}", iso_local_date(zdt), iso_local_time(zdt)),
        BuiltIn::IsoDateTime | BuiltIn::IsoZonedDateTime => format!(
            "{}T{}{offset}{}",
            iso_local_date(zdt),
            iso_local_time(zdt),
            zone_suffix(zdt)
        ),
        BuiltIn::IsoOrdinalDate => format!(
            "{}-{:03}{offset}",
            iso_year(zdt.local.year()),
            zdt.local.date().ordinal()
        ),
        // ISO_WEEK_DATE is the ISO-8601 week, Monday-first with four minimal
        // days -- NOT the locale's week fields that `w`/`Y` use.
        BuiltIn::IsoWeekDate => {
            let week = zdt.local.date().iso_week();
            format!(
                "{}-W{:02}-{}{offset}",
                iso_year(week.year()),
                week.week(),
                zdt.local.date().weekday().num_days_from_monday() + 1
            )
        }
        // RFC 1123 writes `GMT` at zero offset and `+0900` otherwise, and its
        // day-of-month is NOT zero padded.
        BuiltIn::Rfc1123DateTime => {
            let d = zdt.local.date();
            // `appendValue(YEAR, 4, 4, SignStyle.NOT_NEGATIVE)` -- four digits
            // and no sign, so this is the one format with a YEAR RANGE. Both
            // messages are the JDK's; found by the probe sweep, which fed
            // every named format a BC year and a five-digit one.
            if d.year() < 0 {
                return Err(format!(
                    "Field Year cannot be printed as the value {} cannot be negative according to \
                     the SignStyle",
                    d.year()
                ));
            }
            if d.year() > 9999 {
                return Err(format!(
                    "Field Year cannot be printed as the value {} exceeds the maximum print width \
                     of 4",
                    d.year()
                ));
            }
            let t = zdt.local.time();
            let zone = if zdt.offset.local_minus_utc() == 0 {
                "GMT".to_string()
            } else {
                offset_text(zdt.offset, 2, false)
            };
            format!(
                "{}, {} {} {} {:02}:{:02}:{:02} {zone}",
                DAYS_SHORT[d.weekday().num_days_from_monday() as usize],
                d.day(),
                MONTHS_SHORT[d.month0() as usize],
                iso_year(d.year()),
                t.hour(),
                t.minute(),
                t.second()
            )
        }
        BuiltIn::IsoInstant => format_iso_instant(zdt),
    })
}

// ---------------------------------------------------------------------------
// Parsing text into a ZonedDateTime
// ---------------------------------------------------------------------------

/// A cursor over the text being parsed, with Java's own error wording.
struct Scan<'a> {
    text: &'a str,
    chars: Vec<char>,
    at: usize,
}

impl<'a> Scan<'a> {
    fn new(text: &'a str) -> Self {
        Scan {
            text,
            chars: text.chars().collect(),
            at: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    /// The ISO formatters call `parseCaseInsensitive()`, so
    /// `2024-01-15t10:30:00z` parses. **`ofPattern` does not** -- a
    /// user-written `'T'` literal is case-sensitive there, measured -- which
    /// is why this is a second method rather than a change to [`Scan::eat`].
    fn eat_ignoring_case(&mut self, c: char) -> bool {
        if self.peek().is_some_and(|got| got.eq_ignore_ascii_case(&c)) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    /// Reads between `min` and `max` digits, greedily.
    fn digits(&mut self, min: usize, max: usize) -> Option<(u32, usize)> {
        let start = self.at;
        let mut value: u32 = 0;
        while self.at - start < max {
            match self.peek().and_then(|c| c.to_digit(10)) {
                Some(d) => {
                    value = value.checked_mul(10)?.checked_add(d)?;
                    self.at += 1;
                }
                None => break,
            }
        }
        let taken = self.at - start;
        if taken < min {
            self.at = start;
            return None;
        }
        Some((value, taken))
    }

    /// The same, in 64 bits. `N` (nano-of-day) reaches 8.6e13, which does not
    /// fit the `u32` [`Scan::digits`] returns.
    fn digits64(&mut self, min: usize, max: usize) -> Option<(u64, usize)> {
        let start = self.at;
        let mut value: u64 = 0;
        while self.at - start < max {
            match self.peek().and_then(|c| c.to_digit(10)) {
                Some(d) => {
                    value = value.checked_mul(10)?.checked_add(u64::from(d))?;
                    self.at += 1;
                }
                None => break,
            }
        }
        let taken = self.at - start;
        if taken < min {
            self.at = start;
            return None;
        }
        Some((value, taken))
    }

    fn fail(&self) -> String {
        format!(
            "Text '{}' could not be parsed at index {}",
            self.text, self.at
        )
    }

    fn trailing(&self) -> String {
        format!(
            "Text '{}' could not be parsed, unparsed text found at index {}",
            self.text, self.at
        )
    }
}

/// The fields a parse can produce, before they are assembled.
#[derive(Default, Clone)]
struct Parsed {
    /// The PROLEPTIC year, from `u`. Kept apart from `year_of_era` because
    /// `G BC` turns the latter into `1 - value` and must leave the former
    /// alone -- Kestra rejects a pattern that supplies both and disagrees
    /// (`Conflict found: Field Era 1 differs from Era 0`).
    year: Option<i32>,
    year_of_era: Option<i32>,
    /// 1 for AD, 0 for BC, from `G`.
    era: Option<u32>,
    month: Option<u32>,
    day: Option<u32>,
    day_of_year: Option<u32>,
    hour: Option<u32>,
    hour_of_ampm: Option<u32>,
    minute: Option<u32>,
    second: Option<u32>,
    nano: Option<u32>,
    /// `A` and `N` — a whole time in one field, which is why they resolve
    /// where a bare `m` or `s` does not.
    milli_of_day: Option<u32>,
    nano_of_day: Option<u64>,
    /// The fields Java parses **and cross-checks**: they contribute nothing a
    /// full date does not already carry, but a value that disagrees with the
    /// date is a `Conflict found`, not a shrug. See [`Parsed::cross_check`].
    quarter: Option<u32>,
    /// ISO day-of-week, 1 = Monday.
    day_of_week: Option<u32>,
    aligned_week_of_month: Option<u32>,
    modified_julian_day: Option<i64>,
    pm: Option<bool>,
    offset: Option<FixedOffset>,
    zone: Option<Zone>,
}

/// `Parsed.updateCheckConflict` — the JDK's own message, and the mechanism
/// behind two behaviours that look unrelated.
///
/// A field parsed twice with two different values conflicts, and a field
/// **derived** from the resolved date that disagrees with the parsed one
/// conflicts the same way. `java.base/java/time/format/Parsed.java` resolves
/// every auxiliary field into the date and compares.
fn conflict(
    text: &str,
    field: &str,
    derived: impl fmt::Display,
    parsed: impl fmt::Display,
) -> String {
    format!(
        "Text '{text}' could not be parsed: Conflict found: Field {field} {derived} differs from \
         {field} {parsed}"
    )
}

/// Records a parsed value, refusing a **second, different** value for the same
/// field. `uuuu-MM-dd HH:mm dd` on `2024-03-09 14:25 22` is a parse failure in
/// Kestra; a first cut simply took the last occurrence and answered day 22.
fn record<T: Copy + PartialEq + fmt::Display>(
    slot: &mut Option<T>,
    value: T,
    text: &str,
    field: &str,
) -> Result<(), String> {
    if let Some(existing) = *slot {
        if existing != value {
            return Err(conflict(text, field, existing, value));
        }
    }
    *slot = Some(value);
    Ok(())
}

impl Parsed {
    /// The auxiliary fields, checked against the date and time they were
    /// parsed alongside.
    ///
    /// **W2 Task 5's fix round 2 wrote down that Java "parses and then
    /// ignores" these. It does not** — and consuming them silently turned
    /// thirteen probes from agreeing (both engines refused the letter) into
    /// egret answering where Kestra throws, twice with a wrong *time*. The
    /// third review measured that against the pre-fix commit.
    ///
    /// `w`, `W`, `Y`, `e` and `c` are **not** here and are refused at the
    /// field instead: their resolution goes through `WeekFields.of(locale)`,
    /// and `AbstractDate.convert` never passes the template's `locale` to the
    /// parser — so their meaning is the JVM's own region (`WeekFields[MONDAY,1]`
    /// appears verbatim in the harness's conflict messages). Answering them
    /// would be answering a machine-dependent question.
    fn cross_check(
        &self,
        text: &str,
        date: NaiveDate,
        time: Option<NaiveTime>,
    ) -> Result<(), String> {
        if let Some(quarter) = self.quarter {
            let derived = date.month0() / 3 + 1;
            if derived != quarter {
                return Err(conflict(text, "QuarterOfYear", derived, quarter));
            }
        }
        if let Some(dow) = self.day_of_week {
            let derived = date.weekday().number_from_monday();
            if derived != dow {
                return Err(conflict(text, "DayOfWeek", derived, dow));
            }
        }
        if let Some(doy) = self.day_of_year {
            // Only a CHECK when the date came from month-and-day; when it did
            // not, `build` used this as the date's own source.
            if self.month.is_some() && self.day.is_some() && date.ordinal() != doy {
                return Err(conflict(text, "DayOfYear", date.ordinal(), doy));
            }
        }
        if let Some(week) = self.aligned_week_of_month {
            let derived = (date.day() - 1) / 7 + 1;
            if derived != week {
                return Err(conflict(text, "AlignedWeekOfMonth", derived, week));
            }
        }
        if let Some(mjd) = self.modified_julian_day {
            let derived = date
                .signed_duration_since(NaiveDate::from_ymd_opt(1970, 1, 1).expect("a real date"))
                .num_days()
                + 40_587;
            if derived != mjd {
                return Err(format!(
                    "Text '{text}' could not be parsed: Conflict found: Fields resolved to two \
                     different dates"
                ));
            }
        }
        // `A` and `N` carry a whole time, and Java resolves each into
        // HOUR/MINUTE/SECOND/NANO and then cross-checks **field by field,
        // only against the fields that were actually parsed**.
        //
        // Comparing the assembled times instead is too strict, and the
        // refusal-set fixture is what found it: `HH:mm:ss '|' A` round-tripped
        // through the engine's own output carries a millisecond `A` has and
        // `ss` does not, so egret raised a conflict where Kestra simply keeps
        // the millisecond. `A` = 1 against `14:25:36` still conflicts, because
        // there the HOURS differ.
        let whole = self
            .nano_of_day
            .or_else(|| self.milli_of_day.map(|m| u64::from(m) * 1_000_000));
        if let Some(nanos) = whole {
            let seconds = nanos / 1_000_000_000;
            for (parsed, derived, field) in [
                (self.hour, (seconds / 3_600) as u32, "HourOfDay"),
                (self.minute, ((seconds / 60) % 60) as u32, "MinuteOfHour"),
                (self.second, (seconds % 60) as u32, "SecondOfMinute"),
                (self.nano, (nanos % 1_000_000_000) as u32, "NanoOfSecond"),
            ] {
                if let Some(parsed) = parsed {
                    if parsed != derived {
                        return Err(conflict(text, field, parsed, derived));
                    }
                }
            }
        }
        let _ = time;
        Ok(())
    }

    /// Kestra's three-step cascade, collapsed: a parse that produced an
    /// offset is a `ZonedDateTime`; one that produced a date and a time is a
    /// `LocalDateTime.atZone(zone)`; one that produced only a date is a
    /// `LocalDate.atStartOfDay().atZone(zone)`.
    fn build(self, text: &str, zone: &Zone, smart: bool) -> Result<Zdt, String> {
        let unable = || {
            format!("Text '{text}' could not be parsed: Unable to obtain ZonedDateTime from the parsed fields")
        };
        let invalid = || format!("Text '{text}' could not be parsed: Invalid date");
        // `y` is the year OF ERA, so a parsed `G BC` makes the proleptic year
        // `1 - value`; `u` is already proleptic and ignores the era.
        let year = match (self.year, self.year_of_era) {
            // `u` and `G` are both resolved and CROSS-CHECKED: Java refuses
            // `G uuuu-MM-dd` on `BC 2024-01-15` with
            // `Conflict found: Field Era 1 differs from Era 0`, because the
            // proleptic year says AD and the era text says BC.
            (Some(year), _) => {
                let implied = u32::from(year > 0);
                if self.era.is_some_and(|era| era != implied) {
                    return Err(format!(
                        "Text '{text}' could not be parsed: Conflict found: Field Era {implied} \
                         differs from Era {} derived from {year}",
                        self.era.unwrap_or_default()
                    ));
                }
                year
            }
            (None, Some(year_of_era)) => match self.era {
                Some(0) => 1 - year_of_era,
                _ => year_of_era,
            },
            (None, None) => return Err(unable()),
        };
        let date = match (self.month, self.day, self.day_of_year) {
            // **`ResolverStyle.SMART` is `ofPattern`'s default**, and it
            // clamps a day-of-month past the end of its month rather than
            // refusing: `2024-02-30` parses as `2024-02-29`. The bare ISO
            // path is STRICT and does refuse, which is why `smart` is a
            // parameter and not a constant. Measured both ways.
            (Some(month), Some(day), _) => {
                let first = NaiveDate::from_ymd_opt(year, month, 1).ok_or_else(invalid)?;
                match NaiveDate::from_ymd_opt(year, month, day) {
                    Some(date) => date,
                    None if smart && (1..=31).contains(&day) => last_day_of_month(first),
                    None => return Err(invalid()),
                }
            }
            (_, _, Some(ordinal)) => NaiveDate::from_yo_opt(year, ordinal).ok_or_else(invalid)?,
            _ => return Err(unable()),
        };
        // ---------------------------------------------------------------
        // The time, and **whether there is one at all**
        // ---------------------------------------------------------------
        //
        // `Parsed.resolveTimeLenient` in SMART mode only produces a
        // `LocalTime` when the fields form a CONTIGUOUS CHAIN from the hour
        // down. Its shape, from the JDK:
        //
        // ```text
        // hod + moh + som (+ nos)   -> a time
        // hod + moh                 -> a time, seconds 0   (nos must be absent)
        // hod                       -> a time, m/s/n 0     (som and nos absent)
        // anything else             -> NO time
        // ```
        //
        // That is not a detail: it decides whether `AbstractDate.convert`
        // reaches `LocalDateTime.parse` at all, or falls through to
        // `LocalDate.parse(...).atStartOfDay()`. So an `existingFormat` of
        // `uuuu-MM-dd m` **silently discards the minute** and answers
        // midnight, and `uuuu-MM-dd HH s` discards both the hour and the
        // second. Measured across every time letter and every combination;
        // the first cut built a time out of whatever it had and was wrong for
        // fourteen of them.
        let hour = match (self.hour, self.hour_of_ampm, self.pm) {
            (Some(hour), _, _) => Some(hour),
            (None, Some(hour), Some(pm)) => Some(hour % 12 + if pm { 12 } else { 0 }),
            // An `a` with **no time field parsed at all** resolves to the
            // MIDDLE of its half-day: `AM` is 06:00 and `PM` is 18:00.
            // Measured; nothing about the pattern predicts it.
            //
            // **"On its own" is the whole rule and a first cut dropped it.**
            // `Parsed.resolveTimeLenient` guards the midpoint on hour, minute,
            // second AND nano all being absent, so `uuuu MM dd mm a` on
            // `2024 03 09 25 PM` is midnight in Kestra — the synthesised hour
            // never happens, so the chain below never sees one — and was
            // 18:25 here.
            (None, None, Some(pm))
                if self.minute.is_none() && self.second.is_none() && self.nano.is_none() =>
            {
                Some(if pm { 18 } else { 6 })
            }
            (None, None, Some(_)) => None,
            // An hour-of-am-pm with no `a` cannot become an hour-of-day, so
            // there is no time.
            (None, Some(_), None) => None,
            (None, None, None) => None,
        };
        let bad_time = || format!("Text '{text}' could not be parsed: Invalid time");
        let chain = match (hour, self.minute, self.second, self.nano) {
            (Some(h), Some(m), Some(s), n) => {
                Some(NaiveTime::from_hms_nano_opt(h, m, s, n.unwrap_or(0)).ok_or_else(bad_time)?)
            }
            (Some(h), Some(m), None, None) => {
                Some(NaiveTime::from_hms_opt(h, m, 0).ok_or_else(bad_time)?)
            }
            (Some(h), None, None, None) => {
                Some(NaiveTime::from_hms_opt(h, 0, 0).ok_or_else(bad_time)?)
            }
            _ => None,
        };
        self.cross_check(text, date, chain)?;
        let time = if let Some(nanos) = self.nano_of_day {
            Some(
                NaiveTime::from_num_seconds_from_midnight_opt(
                    (nanos / 1_000_000_000) as u32,
                    (nanos % 1_000_000_000) as u32,
                )
                .ok_or_else(bad_time)?,
            )
        } else if let Some(millis) = self.milli_of_day {
            Some(
                NaiveTime::from_num_seconds_from_midnight_opt(
                    millis / 1_000,
                    (millis % 1_000) * 1_000_000,
                )
                .ok_or_else(bad_time)?,
            )
        } else {
            chain
        };

        // **With no time, the parsed offset and zone are DISCARDED too**, and
        // the `timeZone` argument is what the date is placed in. That is the
        // cascade, not a choice: `LocalDate.parse(...).atStartOfDay()` never
        // saw them. `uuuu-MM-dd XXX` on `2024-01-15 Z` is midnight in
        // `timeZone`, not an instant in `Z`.
        let Some(time) = time else {
            return Ok(Zdt::new(date.and_time(NaiveTime::MIN), zone.clone()));
        };
        let local = date.and_time(time);

        match (self.offset, self.zone) {
            (Some(offset), Some(zone)) => Ok(Zdt {
                local,
                offset,
                zone,
            }),
            (Some(offset), None) => Ok(Zdt {
                local,
                offset,
                zone: Zone::Fixed(offset, offset_id(offset)),
            }),
            (None, Some(zone)) => Ok(Zdt::new(local, zone)),
            (None, None) => Ok(Zdt::new(local, zone.clone())),
        }
    }
}

/// The last day of `first`'s month, which is what `ResolverStyle.SMART` clamps
/// an over-long day-of-month to.
fn last_day_of_month(first: NaiveDate) -> NaiveDate {
    first
        .checked_add_months(Months::new(1))
        .and_then(|next| next.pred_opt())
        .unwrap_or(first)
}

/// `ZonedDateTime.parse(text)`, then `LocalDateTime.parse(text).atZone(zone)`,
/// then `LocalDate.parse(text).atStartOfDay().atZone(zone)` — the cascade
/// `AbstractDate.convert` runs when no `existingFormat` is given.
///
/// The grammar is ISO_ZONED_DATE_TIME's, and it is stricter than it looks:
/// `+0700` (no colon) does **not** parse where `+07` does, and a `[Zone]`
/// suffix without an offset in front of it does not parse at all. Both
/// measured through the harness.
pub(super) fn parse_iso(text: &str, zone: &Zone) -> Result<Zdt, String> {
    let mut scan = Scan::new(text);
    let mut parsed = Parsed::default();

    let negative = scan.eat('-');
    let plus = !negative && scan.eat('+');
    let (year, taken) = scan.digits(4, 10).ok_or_else(|| scan.fail())?;
    if plus && taken <= 4 {
        return Err(scan.fail());
    }
    let year = i32::try_from(year).map_err(|_| scan.fail())?;
    parsed.year = Some(if negative { -year } else { year });
    if !scan.eat('-') {
        return Err(scan.fail());
    }
    parsed.month = Some(scan.digits(2, 2).ok_or_else(|| scan.fail())?.0);
    if !scan.eat('-') {
        return Err(scan.fail());
    }
    parsed.day = Some(scan.digits(2, 2).ok_or_else(|| scan.fail())?.0);

    if scan.eat_ignoring_case('T') {
        parsed.hour = Some(scan.digits(2, 2).ok_or_else(|| scan.fail())?.0);
        if !scan.eat(':') {
            return Err(scan.fail());
        }
        parsed.minute = Some(scan.digits(2, 2).ok_or_else(|| scan.fail())?.0);
        if scan.eat(':') {
            parsed.second = Some(scan.digits(2, 2).ok_or_else(|| scan.fail())?.0);
            if scan.eat('.') {
                let (value, taken) = scan.digits(1, 9).ok_or_else(|| scan.fail())?;
                parsed.nano = Some(value * 10u32.pow(9 - taken as u32));
            }
        }
        parse_iso_offset(&mut scan, &mut parsed)?;
    }

    if scan.at != scan.chars.len() {
        return Err(scan.trailing());
    }
    // `LocalDate.parse` is ISO_LOCAL_DATE, whose resolver is STRICT: the ISO
    // path refuses `2024-02-30` where the `ofPattern` path clamps it.
    parsed.build(text, zone, false)
}

/// The offset and the optional `[Zone]` that follows it.
fn parse_iso_offset(scan: &mut Scan<'_>, parsed: &mut Parsed) -> Result<(), String> {
    if scan.eat_ignoring_case('Z') {
        parsed.offset = FixedOffset::east_opt(0);
    } else if matches!(scan.peek(), Some('+') | Some('-')) {
        let sign = if scan.eat('-') {
            -1
        } else {
            scan.eat('+') as i32
        };
        let (h, _) = scan.digits(2, 2).ok_or_else(|| scan.fail())?;
        let mut seconds = (h * 3600) as i32;
        if scan.eat(':') {
            let (m, _) = scan.digits(2, 2).ok_or_else(|| scan.fail())?;
            seconds += (m * 60) as i32;
            if scan.eat(':') {
                let (s, _) = scan.digits(2, 2).ok_or_else(|| scan.fail())?;
                seconds += s as i32;
            }
        }
        parsed.offset = FixedOffset::east_opt(sign * seconds);
        if parsed.offset.is_none() {
            return Err(scan.fail());
        }
    } else {
        return Ok(());
    }

    if scan.eat('[') {
        let start = scan.at;
        while scan.peek().is_some_and(|c| c != ']') {
            scan.at += 1;
        }
        let id: String = scan.chars[start..scan.at].iter().collect();
        if !scan.eat(']') {
            return Err(scan.fail());
        }
        parsed.zone = Some(Zone::resolve(Some(&id))?);
    }
    Ok(())
}

impl Pattern {
    /// Parses `text` against this pattern, strictly: every literal must match
    /// and the whole input must be consumed.
    ///
    /// **Only the fields a date can be rebuilt from are parseable.** `G`, `Q`,
    /// `q`, `w`, `W`, `F`, `A`, `N`, `n`, `g`, `B` and `z` are refused with
    /// the letter named -- reading them would need a resolution pass egret
    /// does not have, and answering *around* them would silently drop part of
    /// the input.
    fn parse_text(&self, text: &str, zone: &Zone) -> Result<Zdt, String> {
        let mut scan = Scan::new(text);
        let mut parsed = Parsed::default();
        self.parse_pieces(&self.pieces, &mut scan, &mut parsed)?;
        if scan.at != scan.chars.len() {
            return Err(scan.trailing());
        }
        parsed.build(text, zone, true)
    }

    fn parse_pieces(
        &self,
        pieces: &[Piece],
        scan: &mut Scan<'_>,
        parsed: &mut Parsed,
    ) -> Result<(), String> {
        let mut i = 0;
        while i < pieces.len() {
            match &pieces[i] {
                Piece::Pad(_) => {
                    while scan.peek() == Some(' ') {
                        scan.at += 1;
                    }
                    i += 1;
                }
                Piece::Literal(text) => {
                    for c in text.chars() {
                        if !scan.eat(c) {
                            return Err(scan.fail());
                        }
                    }
                    i += 1;
                }
                Piece::Field { letter, count } => {
                    parse_field(
                        *letter,
                        *count,
                        subsequent_width(&pieces[i + 1..]),
                        scan,
                        parsed,
                    )?;
                    i += 1;
                }
                // An optional section is attempted and rolled back whole --
                // both the cursor and the fields, so a half-matched section
                // cannot leave a stale value behind.
                Piece::OptionalStart => {
                    let end = matching_end(pieces, i);
                    let saved_at = scan.at;
                    let saved = parsed.clone();
                    if self
                        .parse_pieces(&pieces[i + 1..end], scan, parsed)
                        .is_err()
                    {
                        scan.at = saved_at;
                        *parsed = saved;
                    }
                    i = end + 1;
                }
                Piece::OptionalEnd => i += 1,
            }
        }
        Ok(())
    }
}

/// **Java's adjacent value parsing**, which is what lets `yyyyMMdd` parse
/// `20240115` at all.
///
/// `appendValue(field, minWidth, maxWidth, signStyle)` with `minWidth !=
/// maxWidth` is variable-width, and `yyyy` is `appendValue(YEAR, 4, 19, …)`.
/// On its own it would swallow all eight digits. Java instead lets each
/// *fixed*-width field that immediately follows reserve its own width, and the
/// variable field then keeps `parsed_len - subsequent_width` digits.
///
/// This returns that reserved width: the fixed widths of the run of numeric
/// fields that follows, stopping at the first literal, optional marker, pad,
/// text-style field or second variable-width field — which is where Java's
/// active-parsing mode ends too.
///
/// Found by the fix round's review: `yyyyMMdd` is one of the commonest date
/// formats in the wild and did not parse.
fn subsequent_width(rest: &[Piece]) -> usize {
    let mut total = 0;
    for piece in rest {
        match piece {
            Piece::Field { letter, count } => match fixed_parse_width(*letter, *count) {
                Some(width) => total += width,
                None => break,
            },
            _ => break,
        }
    }
    total
}

/// The width a field consumes when it is fixed-width for *parsing*, or `None`
/// when it is variable-width or not numeric.
fn fixed_parse_width(letter: char, count: usize) -> Option<usize> {
    match letter {
        // `yy`/`uu`/`YY` are `appendValueReduced(field, 2, 2, …)`: fixed at 2.
        'y' | 'u' | 'Y' if count == 2 => Some(2),
        // Every other year count is variable-width.
        'y' | 'u' | 'Y' => None,
        'M' | 'L' | 'Q' | 'q' | 'E' | 'e' | 'c' if count > 2 => None,
        'M' | 'L' | 'd' | 'D' | 'H' | 'h' | 'k' | 'K' | 'm' | 's' | 'S' | 'Q' | 'q' | 'e' | 'c'
        | 'w' | 'W' | 'F' | 'A' | 'N' | 'n' | 'g'
            if count >= 2 =>
        {
            Some(count)
        }
        _ => None,
    }
}

fn matching_end(pieces: &[Piece], start: usize) -> usize {
    let mut depth = 0usize;
    for (i, piece) in pieces.iter().enumerate().skip(start) {
        match piece {
            Piece::OptionalStart => depth += 1,
            Piece::OptionalEnd => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    pieces.len()
}

fn parse_field(
    letter: char,
    count: usize,
    subsequent: usize,
    scan: &mut Scan<'_>,
    parsed: &mut Parsed,
) -> Result<(), String> {
    // A field written once parses one or two digits; written `n` times it
    // parses exactly `n`, which is what makes `yyyy` refuse a two-digit year.
    //
    // **The range check is `ChronoField.checkValidValue`, and it is not
    // optional.** `KK:mm a` on `12:30 PM` is
    // `Invalid value for HourOfAmPm (valid values 0 - 11): 12` in Kestra;
    // without the check egret answered 12:30. Found by the fix round's
    // `existingFormat` sweep, which fed each field a value one past its range.
    let numeric = |scan: &mut Scan<'_>,
                   natural: usize,
                   field: &str,
                   lo: u32,
                   hi: u32|
     -> Result<u32, String> {
        let (min, max) = if count == 1 {
            (1, natural)
        } else {
            (count, count)
        };
        let value = scan
            .digits(min, max)
            .map(|(value, _)| value)
            .ok_or_else(|| scan.fail())?;
        if !(lo..=hi).contains(&value) {
            return Err(format!(
                "Text '{}' could not be parsed: Invalid value for {field} (valid values {lo} - \
                 {hi}): {value}",
                scan.text
            ));
        }
        Ok(value)
    };
    match letter {
        'y' | 'u' => {
            let value = if count == 2 {
                // `appendValueReduced(field, 2, 2, 2000)`.
                i32::from(scan.digits(2, 2).ok_or_else(|| scan.fail())?.0 as i16) + 2000
            } else {
                let negative = scan.eat('-');
                // **`SignStyle.EXCEEDS_PAD` reads back the way it writes**:
                // a `+` is REQUIRED when the value needs more digits than the
                // pad and forbidden otherwise. So `uuuu` takes `+10000` and
                // `uuuuu` -- whose width already holds five digits -- does
                // not. Without this an `iso` round trip of a year past 9999
                // failed on its second leg, the other half of fix round 1's
                // EXCEEDS_PAD fix.
                let positive = !negative && scan.eat('+');
                let start = scan.at;
                let (_, taken) = scan.digits(count.max(1), 10).ok_or_else(|| scan.fail())?;
                let keep = taken.saturating_sub(subsequent).max(count.max(1));
                if keep > taken {
                    return Err(scan.fail());
                }
                scan.at = start;
                let value = scan.digits(keep, keep).ok_or_else(|| scan.fail())?.0;
                let value = i32::try_from(value).map_err(|_| scan.fail())?;
                if positive && (count < 4 || keep <= count) {
                    return Err(scan.fail());
                }
                if negative {
                    -value
                } else {
                    value
                }
            };
            if letter == 'u' {
                record(&mut parsed.year, value, scan.text, "Year")?;
            } else {
                // YearOfEra is 1-based and cannot be zero or negative, so
                // `yyyy-MM-dd` refuses `-0044-01-15` and `0000-01-15` where
                // `uuuu-MM-dd` takes both.
                if value < 1 {
                    return Err(format!(
                        "Text '{}' could not be parsed: Invalid value for YearOfEra (valid values \
                         1 - 999999999/1000000000): {value}",
                        scan.text
                    ));
                }
                record(&mut parsed.year_of_era, value, scan.text, "YearOfEra")?;
            }
        }
        // `G` — the era, which is what turns a `y` year-of-era into a
        // negative proleptic year. `GG yyyy-MM-dd` on `AD 2024-01-15` parses
        // in Kestra, measured, and a first cut refused the letter outright.
        'G' => {
            let index = match count {
                5 => eat_one_of(scan, &["A", "B"]),
                4 => eat_one_of(scan, &["Anno Domini", "Before Christ"]),
                _ => eat_one_of(scan, &["AD", "BC"]),
            }
            .ok_or_else(|| scan.fail())?;
            parsed.era = Some(if index == 0 { 1 } else { 0 });
        }
        'M' | 'L' if count <= 2 => {
            let value = numeric(scan, 2, "MonthOfYear", 1, 12)?;
            record(&mut parsed.month, value, scan.text, "MonthOfYear")?;
        }
        'M' | 'L' if count == 3 || count == 4 => {
            let names: &[&str; 12] = if count == 3 {
                &MONTHS_SHORT
            } else {
                &MONTHS_FULL
            };
            let index = eat_one_of(scan, names).ok_or_else(|| scan.fail())?;
            record(
                &mut parsed.month,
                index as u32 + 1,
                scan.text,
                "MonthOfYear",
            )?;
        }
        'd' => {
            let value = numeric(scan, 2, "DayOfMonth", 1, 31)?;
            record(&mut parsed.day, value, scan.text, "DayOfMonth")?;
        }
        'D' => {
            let value = numeric(scan, 3, "DayOfYear", 1, 366)?;
            record(&mut parsed.day_of_year, value, scan.text, "DayOfYear")?;
        }
        'H' => {
            let value = numeric(scan, 2, "HourOfDay", 0, 23)?;
            record(&mut parsed.hour, value, scan.text, "HourOfDay")?;
        }
        // `ClockHourOfDay`'s range is 1-24, but **SMART accepts 0** and reads
        // it as hour 0 -- so `kk:mm` takes `00:30`. Measured.
        'k' => parsed.hour = Some(numeric(scan, 2, "ClockHourOfDay", 0, 24)? % 24),
        // Same SMART exemption as `k` one line up, and it was missing here:
        // `ClockHourOfAmPm`'s range is 1-12, but SMART accepts 0 and reads it
        // as 0. `hh:mm a` on `00:30 AM` was byte-exact before fix round 2 and
        // an error after it -- the one probe in 5528 that regressed from
        // right to an error.
        'h' => parsed.hour_of_ampm = Some(numeric(scan, 2, "ClockHourOfAmPm", 0, 12)? % 12),
        'K' => parsed.hour_of_ampm = Some(numeric(scan, 2, "HourOfAmPm", 0, 11)?),
        'm' => {
            let value = numeric(scan, 2, "MinuteOfHour", 0, 59)?;
            record(&mut parsed.minute, value, scan.text, "MinuteOfHour")?;
        }
        's' => {
            let value = numeric(scan, 2, "SecondOfMinute", 0, 59)?;
            record(&mut parsed.second, value, scan.text, "SecondOfMinute")?;
        }
        'S' => {
            let value = scan.digits(count, count).ok_or_else(|| scan.fail())?.0;
            parsed.nano = Some(value * 10u32.pow(9 - count as u32));
        }
        // `A`, `N`, `n` and `g` are `appendValue(field, count, 19, …)` —
        // **variable**-width, minimum `count`. Reading them at exactly `count`
        // digits leaves the rest of the number as unparsed text.
        'n' => {
            let (value, _) = scan.digits64(count.max(1), 19).ok_or_else(|| scan.fail())?;
            parsed.nano = Some(u32::try_from(value).map_err(|_| scan.fail())?);
        }
        // `A` and `N` carry a whole time in one field, which is why an
        // `existingFormat` of `uuuu-MM-dd A` resolves where `uuuu-MM-dd m`
        // does not. See [`Parsed::build`].
        'A' => {
            let (value, _) = scan.digits64(count.max(1), 19).ok_or_else(|| scan.fail())?;
            parsed.milli_of_day = Some(u32::try_from(value).map_err(|_| scan.fail())?);
        }
        'N' => {
            let (value, _) = scan.digits64(count.max(1), 19).ok_or_else(|| scan.fail())?;
            parsed.nano_of_day = Some(value);
        }
        'a' => {
            let index = eat_one_of(scan, &["AM", "PM"]).ok_or_else(|| scan.fail())?;
            parsed.pm = Some(index == 1);
        }
        // Day names carry no information the date does not already have;
        // Java cross-checks them and egret consumes them. **`E` is text at
        // EVERY count** -- `E`, `EE` and `EEE` all print `Mon` -- which the
        // first cut missed by requiring `count >= 3` for the whole group.
        // `E` is the ISO day-of-week as text, and it is CROSS-CHECKED
        // against the date -- `uuuu-MM-dd EEEE` on `2024-03-09 Monday` is a
        // conflict, because that date is a Saturday.
        'E' if count == 5 => {
            let index = eat_one_of(scan, &DAYS_NARROW).ok_or_else(|| scan.fail())?;
            record(
                &mut parsed.day_of_week,
                index as u32 + 1,
                scan.text,
                "DayOfWeek",
            )?;
        }
        'E' => {
            let index = eat_one_of(scan, &DAYS_FULL)
                .or_else(|| eat_one_of(scan, &DAYS_SHORT))
                .ok_or_else(|| scan.fail())?;
            record(
                &mut parsed.day_of_week,
                index as u32 + 1,
                scan.text,
                "DayOfWeek",
            )?;
        }
        'Q' | 'q' if count <= 2 => {
            let value = numeric(scan, 2, "QuarterOfYear", 1, 4)?;
            record(&mut parsed.quarter, value, scan.text, "QuarterOfYear")?;
        }
        'Q' | 'q' if count == 3 => {
            let index = eat_one_of(scan, &["Q1", "Q2", "Q3", "Q4"]).ok_or_else(|| scan.fail())?;
            record(
                &mut parsed.quarter,
                index as u32 + 1,
                scan.text,
                "QuarterOfYear",
            )?;
        }
        'Q' | 'q' if count == 4 => {
            let index = eat_one_of(scan, &QUARTERS_FULL).ok_or_else(|| scan.fail())?;
            record(
                &mut parsed.quarter,
                index as u32 + 1,
                scan.text,
                "QuarterOfYear",
            )?;
        }
        'Q' | 'q' => {
            let (value, _) = scan.digits(1, 1).ok_or_else(|| scan.fail())?;
            record(&mut parsed.quarter, value, scan.text, "QuarterOfYear")?;
        }
        'F' => {
            let value = numeric(scan, 1, "AlignedWeekOfMonth", 1, 5)?;
            record(
                &mut parsed.aligned_week_of_month,
                value,
                scan.text,
                "AlignedWeekOfMonth",
            )?;
        }
        'g' => {
            let negative = scan.eat('-');
            let (value, _) = scan.digits64(count.max(1), 19).ok_or_else(|| scan.fail())?;
            let value = i64::try_from(value).map_err(|_| scan.fail())?;
            record(
                &mut parsed.modified_julian_day,
                if negative { -value } else { value },
                scan.text,
                "ModifiedJulianDay",
            )?;
        }
        // **`w`, `W`, `Y`, `e` and `c` are refused, and that is a narrowing
        // from fix round 2 rather than an omission.** All five resolve through
        // `WeekFields.of(locale)`, and `AbstractDate.convert` never passes the
        // template's `locale` to the parser -- the harness's own conflict
        // messages read `WeekFields[MONDAY,1]`, which is the JVM's region and
        // not the entry's argument. Consuming them silently (round 2) made
        // egret answer a machine-dependent question; giving them meaning would
        // make egret answer it *differently* on another host. Refusing says
        // so.
        'w' | 'W' | 'Y' | 'e' | 'c' => {
            return Err(format!(
                "the 'date' filters cannot parse an 'existingFormat' containing pattern letter \
                 '{letter}': it resolves through the JVM's own locale, which Kestra does not pass \
                 to the parser, so its meaning is the host's region"
            ))
        }
        'X' | 'x' | 'Z' | 'O' => {
            parsed.offset = Some(parse_offset_field(letter, count, scan)?);
        }
        'V' => {
            let start = scan.at;
            while scan
                .peek()
                .is_some_and(|c| c.is_ascii_alphanumeric() || "/_+-.:".contains(c))
            {
                scan.at += 1;
            }
            let id: String = scan.chars[start..scan.at].iter().collect();
            let zone = Zone::resolve(Some(&id))?;
            // **The offset stays.** `ZonedDateTime.from` computes
            // `INSTANT_SECONDS` from the parsed OFFSET and uses the zone for
            // display only, so clearing it here moved the instant by the
            // difference between the two -- seven hours for
            // `+02:00 America/New_York` -- and only when the offset was parsed
            // FIRST, which is why no single-letter round trip saw it.
            parsed.zone = Some(zone);
        }
        // The same three letters the FORMAT side refuses, for the same
        // reason: their text lives in a locale database. Keeping the two sets
        // identical is deliberate -- a letter egret can write it can read.
        other => {
            return Err(format!(
                "the 'date' filters cannot parse an 'existingFormat' containing pattern letter \
                 '{other}': it needs the locale's zone/day-period names, which egret does not carry"
            ))
        }
    }
    Ok(())
}

/// `X`, `x`, `Z` and `O` on the PARSE side, where each count accepts its own
/// shape and nothing else.
///
/// The first cut routed all four through the ISO offset parser, which accepts
/// every shape — so `XXX` took `+07` and lowercase `z`, both of which Java
/// refuses (`ofPattern` is case-sensitive, unlike the ISO constants).
fn parse_offset_field(
    letter: char,
    count: usize,
    scan: &mut Scan<'_>,
) -> Result<FixedOffset, String> {
    // `ZZZZ` and `O` are the LOCALIZED GMT forms; everything else is an
    // ISO-8601 offset whose separator and width follow the count.
    if letter == 'O' || (letter == 'Z' && count == 4) {
        if !scan.eat('G') || !scan.eat('M') || !scan.eat('T') {
            return Err(scan.fail());
        }
        if !matches!(scan.peek(), Some('+') | Some('-')) {
            return Ok(FixedOffset::east_opt(0).expect("0 is a valid offset"));
        }
        // The hour is one digit under `O` (`GMT+7`) and two under `OOOO` /
        // `ZZZZ` (`GMT+07:00`); a min of 1 and a max of 2 takes both.
        return read_offset(scan, true, 2, true);
    }
    // `X` writes `Z` at a zero offset; `x` and `Z` never do.
    if letter == 'X' && scan.eat('Z') {
        return Ok(FixedOffset::east_opt(0).expect("0 is a valid offset"));
    }
    if letter == 'Z' && count == 5 && scan.eat('Z') {
        return Ok(FixedOffset::east_opt(0).expect("0 is a valid offset"));
    }
    let (colon, minutes_optional) = match (letter, count) {
        ('Z', 1..=3) => (false, false),
        ('Z', _) => (true, false),
        (_, 1) => (false, true),
        (_, 2) => (false, false),
        (_, 3) => (true, false),
        (_, 4) => (false, false),
        _ => (true, false),
    };
    read_offset(scan, colon, 2, minutes_optional)
}

/// Reads `±HH[[:]MM[[:]SS]]` with the separator and the hour width fixed by
/// the caller.
fn read_offset(
    scan: &mut Scan<'_>,
    colon: bool,
    hour_width: usize,
    minutes_optional: bool,
) -> Result<FixedOffset, String> {
    let sign = if scan.eat('-') {
        -1
    } else if scan.eat('+') {
        1
    } else {
        return Err(scan.fail());
    };
    let (hours, _) = scan.digits(1, hour_width).ok_or_else(|| scan.fail())?;
    let mut seconds = (hours * 3600) as i32;
    let saved = scan.at;
    let read_pair = |scan: &mut Scan<'_>| -> Option<u32> {
        let saved = scan.at;
        if colon && !scan.eat(':') {
            scan.at = saved;
            return None;
        }
        match scan.digits(2, 2) {
            Some((value, _)) => Some(value),
            None => {
                scan.at = saved;
                None
            }
        }
    };
    match read_pair(scan) {
        Some(minutes) => {
            seconds += (minutes * 60) as i32;
            if let Some(secs) = read_pair(scan) {
                seconds += secs as i32;
            }
        }
        None if minutes_optional => scan.at = saved,
        None => return Err(scan.fail()),
    }
    FixedOffset::east_opt(sign * seconds).ok_or_else(|| scan.fail())
}

/// Consumes the first of `names` that matches at the cursor, longest first so
/// `January` is not shadowed by `Jan`.
fn eat_one_of(scan: &mut Scan<'_>, names: &[&str]) -> Option<usize> {
    let rest: String = scan.chars[scan.at..].iter().collect();
    let mut best: Option<(usize, usize)> = None;
    for (index, name) in names.iter().enumerate() {
        if rest.starts_with(name) && best.is_none_or(|(_, len)| name.len() > len) {
            best = Some((index, name.len()));
        }
    }
    let (index, len) = best?;
    scan.at += names[index].chars().count();
    let _ = len;
    Some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ZoneId.of` accepts five different shapes and they do **not** all
    /// produce the same kind of zone: an offset has no transitions and prints
    /// its own id under `VV`, a prefixed offset prints the prefix, and a
    /// region prints its tzdb name. Measured against the JDK for each row.
    #[test]
    fn zone_ids_are_javas_five_shapes() {
        for (input, id) in [
            ("Z", "Z"),
            ("+05:30", "+05:30"),
            ("+05", "+05:00"),
            ("-0330", "-03:30"),
            ("UTC", "UTC"),
            ("GMT", "GMT"),
            ("GMT+7", "GMT+07:00"),
            ("UT+5", "UT+05:00"),
            ("UTC+00:00", "UTC"),
            ("Etc/UTC", "Etc/UTC"),
            ("Etc/GMT+7", "Etc/GMT+7"),
            ("Asia/Tokyo", "Asia/Tokyo"),
        ] {
            let zone = Zone::resolve(Some(input))
                .unwrap_or_else(|e| panic!("{input} should resolve: {e}"));
            assert_eq!(zone.id(), id, "{input}");
        }
        // Case-sensitive, exactly like `ZoneId.of`.
        assert!(Zone::resolve(Some("utc")).is_err());
        assert!(Zone::resolve(Some("Not/AZone")).is_err());
    }

    /// The three `ZonedDateTime.ofLocal` cases. The **gap** is the one worth a
    /// test of its own: Java moves the local time forward by the transition's
    /// own duration, so 02:30 on a spring-forward day is 03:30, not 03:00 —
    /// and `chrono` alone only tells you the local time does not exist.
    #[test]
    fn a_local_time_in_a_dst_gap_moves_forward_by_the_gaps_duration() {
        let ny = Zone::resolve(Some("America/New_York")).expect("a real zone");
        let gap = NaiveDate::from_ymd_opt(2024, 3, 10)
            .and_then(|d| d.and_hms_opt(2, 30, 0))
            .expect("a real local time");
        let (local, offset) = ny.resolve_local(gap);
        assert_eq!(local.time(), NaiveTime::from_hms_opt(3, 30, 0).unwrap());
        assert_eq!(offset.local_minus_utc(), -4 * 3600);

        // An overlap keeps the EARLIER offset, which is Java's rule too.
        let overlap = NaiveDate::from_ymd_opt(2024, 11, 3)
            .and_then(|d| d.and_hms_opt(1, 30, 0))
            .expect("a real local time");
        assert_eq!(ny.resolve_local(overlap).1.local_minus_utc(), -4 * 3600);
    }

    /// **The alphabet is asserted against the ENGINE, not against a comment.**
    ///
    /// `oracle/letter-counts.txt` records, per ASCII letter, which counts
    /// `DateTimeFormatter.ofPattern` accepts — generated by pushing every
    /// letter at counts 1-11 through the real Pebble engine. This checks
    /// [`letter_limits`] against it **in both directions**: a count the engine
    /// takes must compile here, and a count it rejects must not.
    ///
    /// It exists because two rows were wrong and nothing noticed. `v` was
    /// missing from the table entirely, so egret answered
    /// `Unknown pattern letter: v` for a pattern Java compiles; and `B`
    /// accepted counts 2 and 3 that Java rejects. Both were found by a
    /// scratchpad script — which is not reproducible and not reviewable, so
    /// the table it produced is committed and this is what reads it.
    #[test]
    fn the_pattern_alphabet_matches_the_engines_own() {
        let fixture = include_str!("../../oracle/letter-counts.txt");
        let mut rows = 0;
        for line in fixture.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (letter, counts) = line.split_once(' ').expect("`<letter> <counts>`");
            let letter: char = letter.chars().next().expect("a letter");
            let accepted: Vec<usize> = if counts.trim() == "-" {
                Vec::new()
            } else {
                counts
                    .split(',')
                    .map(|c| c.trim().parse().expect("a count"))
                    .collect()
            };
            rows += 1;
            for count in 1..=11usize {
                let pattern: String = std::iter::repeat_n(letter, count).collect();
                let compiled = Pattern::parse(&pattern).is_ok();
                assert_eq!(
                    compiled,
                    accepted.contains(&count),
                    "`{pattern}`: the engine {} it, egret {} it",
                    if accepted.contains(&count) {
                        "takes"
                    } else {
                        "refuses"
                    },
                    if compiled { "takes" } else { "refuses" },
                );
            }
        }
        // Every ASCII letter, upper and lower, so a row cannot go missing.
        assert_eq!(rows, 52, "the fixture must cover all 52 ASCII letters");
    }

    /// **The refusal sets are asserted against the ENGINE, per side, in both
    /// directions.**
    ///
    /// [`the_pattern_alphabet_matches_the_engines_own`] covers only whether a
    /// pattern *compiles*. `z` compiles and then refuses at render; `w`
    /// compiles and then refuses at parse. That layer had no fixture, and it
    /// is where `v` and the four legacy zone ids lived — both found by a
    /// reviewer running probes, neither by the suite.
    ///
    /// `oracle/refusal-sets.txt` records, per letter and per side, what the
    /// real engine does and what egret must do, with the engine column
    /// measured through the harness. This checks:
    ///
    /// - egret's actual outcome equals the declared one, **naming the letter
    ///   and the side** when it does not — so widening or narrowing a refusal
    ///   fails by name;
    /// - a row whose two outcomes differ carries a reason and a row whose
    ///   outcomes agree does not — so a divergence cannot be added silently,
    ///   and a refusal cannot claim to be one when the engine refuses too;
    /// - every letter that compiles appears on **both** sides, so a new letter
    ///   cannot enter the alphabet undeclared.
    ///
    /// The two sides differ deliberately — three refusals when writing, eight
    /// when reading — and the file encodes that rather than flattening it.
    /// Writing this fixture immediately found an undeclared divergence of its
    /// own: `A` and `N` were cross-checked against the assembled time instead
    /// of field by field, so a round trip through the engine's own output
    /// raised a conflict Kestra does not.
    #[test]
    fn the_refusal_sets_match_the_engines_own_per_side() {
        let alphabet = include_str!("../../oracle/letter-counts.txt");
        let mut compiling: Vec<char> = Vec::new();
        for line in alphabet.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (letter, counts) = line.split_once(' ').expect("`<letter> <counts>`");
            // `p` is a pad and never stands alone; it has its own test.
            if counts.trim() != "-" && letter != "p" {
                compiling.push(letter.chars().next().expect("a letter"));
            }
        }

        let fixture = include_str!("../../oracle/refusal-sets.txt");
        let mut seen: Vec<(String, char)> = Vec::new();
        for line in fixture.lines() {
            let line = line.trim_end();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let cols: Vec<&str> = line.split('\t').collect();
            assert_eq!(cols.len(), 6, "six tab-separated columns: {line}");
            let (side, pattern, engine, declared, sample, reason) =
                (cols[0], cols[1], cols[2], cols[3], cols[4], cols[5]);
            let letter = pattern.chars().next().expect("a pattern");
            seen.push((side.to_string(), letter));

            // A divergence must be explained, and a non-divergence must not
            // pretend to be one.
            if engine == declared {
                assert_eq!(
                    reason, "-",
                    "{side} `{pattern}`: the engine and egret agree, so there is nothing to \
                     explain -- a row that carries a reason here is claiming a divergence it \
                     does not have"
                );
            } else {
                assert!(
                    reason.len() > 20,
                    "{side} `{pattern}`: the engine says {engine} and egret says {declared}; a \
                     divergence must say why, in the file"
                );
            }

            let (template, context) = match side {
                "format" => (
                    format!("{{{{ d | date('{pattern}', timeZone='UTC', locale='en') }}}}"),
                    r#"{"d": "2024-03-09T14:25:36.123Z"}"#.to_string(),
                ),
                "parse" => (
                    format!(
                        "{{{{ d | date('uuuu-MM-dd HH:mm:ss', timeZone='UTC', \
                         existingFormat=\"uuuu-MM-dd HH:mm:ss '|' {pattern}\") }}}}"
                    ),
                    format!(r#"{{"d": "2024-03-09 14:25:36 | {sample}"}}"#),
                ),
                other => panic!("unknown side `{other}`"),
            };
            let cx = super::super::env::RenderContext::from_json_str(&context)
                .expect("context must parse");
            let actual = if super::super::env::render_str(&template, &cx).is_ok() {
                "ok"
            } else {
                "refuse"
            };
            assert_eq!(
                actual, declared,
                "{side} `{pattern}`: the fixture declares {declared} and egret says {actual} \
                 (the engine says {engine})"
            );
        }

        // Both sides, every compiling letter, no gaps.
        for letter in compiling {
            for side in ["format", "parse"] {
                assert!(
                    seen.contains(&(side.to_string(), letter)),
                    "`{letter}` compiles but has no {side} row: a letter cannot enter the \
                     alphabet without declaring what egret does with it"
                );
            }
        }
    }

    /// A pattern is parsed once into pieces, and the piece boundaries are
    /// where the quoting rules actually live. `''` is a quote whether it
    /// stands alone or sits inside a longer literal — the second case is what
    /// mutation testing found missing from the corpus.
    #[test]
    fn quoting_resolves_at_parse_time() {
        let pieces = |p: &str| Pattern::parse(p).expect("valid").pieces;
        assert_eq!(pieces("'It''s'"), vec![Piece::Literal("It's".to_string())]);
        assert_eq!(pieces("''"), vec![Piece::Literal("'".to_string())]);
        assert_eq!(pieces("'T'"), vec![Piece::Literal("T".to_string())]);
        assert_eq!(
            pieces("y"),
            vec![Piece::Field {
                letter: 'y',
                count: 1
            }]
        );
    }
}
