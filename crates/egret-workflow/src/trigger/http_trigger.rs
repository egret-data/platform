// kestra: core/src/main/java/io/kestra/plugin/core/http/Trigger.java

//! `http.Trigger` -- a polling trigger (package 11e, Task 2: model, parse and
//! interval scheduling; the poll itself is Task 3).
//!
//! Measured at the pinned checkout (Task 0 report §2): the class is a
//! `PollingTriggerInterface`, not `Schedulable`. It does not override
//! `nextEvaluationDate`, so the next evaluation is `now + interval`, with
//! `interval` defaulting to 60 seconds. There is no backlog of missed ticks and
//! it never runs out. It writes no `scheduled_date` (not a date tick).

use chrono::TimeDelta;

use crate::models::HttpTrigger;

use super::duration::{parse_duration, DurationError};

/// Java's `Duration interval = Duration.ofSeconds(60)`.
pub const DEFAULT_INTERVAL: TimeDelta = TimeDelta::seconds(60);

/// The poll cadence. A zero or negative interval would re-claim the row on
/// every pass, so it is refused rather than honoured.
pub fn interval(t: &HttpTrigger) -> Result<TimeDelta, String> {
    let Some(raw) = t.interval.as_deref() else {
        return Ok(DEFAULT_INTERVAL);
    };
    let d = parse_duration(raw).map_err(|e: DurationError| format!("interval: {e}"))?;
    if d <= TimeDelta::zero() {
        return Err(format!("interval: `{raw}` must be longer than zero"));
    }
    Ok(d)
}

/// What egret will not honour on this trigger, each naming the package that
/// would lift it (Task 0 §2). Empty when the trigger is acceptable.
pub fn refusals(t: &HttpTrigger) -> Vec<String> {
    let mut out = Vec::new();
    if t.options.is_some() {
        out.push(
            "http.Trigger `options` (proxy, ssl, auth, timeouts) is not supported; package #4's \
             http.Request refusals apply and the request configuration is not modelled for \
             triggers until the trigger poll lifts it"
                .to_string(),
        );
    }
    if t.encrypt_body.is_some() {
        out.push(
            "http.Trigger `encryptBody` is not supported (it needs Kestra's key-encrypted \
             outputs, which egret does not model)"
                .to_string(),
        );
    }
    if t.form_data.is_some() {
        out.push(
            "http.Trigger `formData` is not supported; multipart bodies belong to package #4's \
             http.Request, not the trigger poll"
                .to_string(),
        );
    }
    if let Err(e) = interval(t) {
        out.push(e);
    }
    out
}

/// `spec_key`: the fields that decide the cadence and the request, so an edit
/// to any re-initialises the row (`now + interval`).
pub fn spec_key(t: &HttpTrigger) -> String {
    let p = |o: &Option<crate::models::Property<String>>| {
        o.as_ref()
            .map(|v| v.source().to_string())
            .unwrap_or_default()
    };
    let v = |o: &Option<serde_json::Value>| o.as_ref().map(|x| x.to_string()).unwrap_or_default();
    format!(
        "http|{}|{}|{}|{}|{}|{}|{}|{}",
        p(&t.uri),
        p(&t.method),
        p(&t.body),
        v(&t.params),
        v(&t.headers),
        p(&t.content_type),
        p(&t.response_condition),
        t.interval.as_deref().unwrap_or("")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TriggerKind;

    fn parse(extra: &str) -> HttpTrigger {
        let yaml = format!(
            "id: h\ntype: io.kestra.plugin.core.http.Trigger\nuri: https://x.test\n{extra}"
        );
        let t: crate::models::Trigger = serde_norway::from_str(&yaml).unwrap();
        let TriggerKind::HttpTrigger(h) = t.kind else {
            panic!("http trigger")
        };
        *h
    }

    #[test]
    fn interval_defaults_to_sixty_seconds_and_overrides() {
        assert_eq!(interval(&parse("")).unwrap(), TimeDelta::seconds(60));
        assert_eq!(
            interval(&parse("interval: PT30S\n")).unwrap(),
            TimeDelta::seconds(30)
        );
        assert!(interval(&parse("interval: PT0S\n")).is_err());
        assert!(interval(&parse("interval: nonsense\n")).is_err());
    }

    #[test]
    fn unmodelled_keys_are_refused_with_their_package() {
        assert!(refusals(&parse("")).is_empty());
        let r = refusals(&parse("options: {}\nencryptBody: true\nformData: {a: b}\n"));
        assert_eq!(r.len(), 3, "{r:?}");
        assert!(r.iter().any(|m| m.contains("options") && m.contains("#4")));
        assert!(r.iter().any(|m| m.contains("encryptBody")));
        assert!(r.iter().any(|m| m.contains("formData") && m.contains("#4")));
    }

    #[test]
    fn the_spec_key_follows_the_cadence_and_request() {
        assert_ne!(spec_key(&parse("")), spec_key(&parse("interval: PT30S\n")));
        assert_eq!(spec_key(&parse("")), spec_key(&parse("")));
    }
}
