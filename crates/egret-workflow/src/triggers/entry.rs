//! Every field rule of a trigger entry (W7e spec §3.5), pure: a flow row, its
//! parsed draft, the state rows and the caller's `workflow:manage` in,
//! entries out. No I/O, so each rule is tested without a database.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::routes::{
    ConditionDto, DependsOnDto, FlowTriggerInfoDto, MissedTicksDto, ScheduleInfoDto, ToggleRefusal,
    TriggerEntryDto, TriggerKindDto, WebhookInfoDto, WindowDto,
};
use super::state::{Key, StateRows};
use crate::models::{Flow, State, Trigger, TriggerKind};
use crate::trigger::{
    condition_key, KeyContext, ScheduleTrigger, TimeWindow, WindowFreeFlowTrigger,
};

/// The most missed ticks counted; beyond it the entry says "more".
pub(crate) const MISSED_TICKS_CAP: u32 = 1_000;

/// The flow row an entry belongs to, as the routes read it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FlowCtx<'a> {
    pub id: Uuid,
    /// The workspace's slug: the webhook path's first segment.
    pub ws_slug: &'a str,
    /// The `namespace` and `flow_id` COLUMNS: what the webhook route matches
    /// its path against.
    pub namespace: &'a str,
    pub flow_id: &'a str,
    /// The `state` column (`draft`, `live`, `disabled`). `disabled` stops
    /// every trigger at fire time (the schedule claim, the listener select,
    /// the webhook's 409), so it makes every entry effective-disabled.
    pub state: &'a str,
    /// In the trash (`deleted_at IS NOT NULL`).
    pub deleted: bool,
}

/// How much an entry carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Detail {
    /// The per-flow route: everything, missed ticks included.
    Full,
    /// A row of the workspace list: no `depends_on.conditions`, no `webhook`,
    /// and no missed ticks (the list counts them for its page only, with
    /// [`missed_ticks`]).
    Row,
}

/// One entry per declared trigger of `parsed`, in declaration order.
pub(crate) fn entries_for_flow(
    flow: &FlowCtx,
    parsed: &Flow,
    state: &StateRows,
    may_manage: bool,
    detail: Detail,
) -> Vec<TriggerEntryDto> {
    parsed
        .triggers
        .iter()
        .map(|t| entry(flow, parsed, t, state, may_manage, detail))
        .collect()
}

fn entry(
    flow: &FlowCtx,
    parsed: &Flow,
    t: &Trigger,
    state: &StateRows,
    may_manage: bool,
    detail: Detail,
) -> TriggerEntryDto {
    let key: Key = (flow.id, t.id.clone());
    let kind = match &t.kind {
        TriggerKind::Schedule(_) => TriggerKindDto::Schedule,
        TriggerKind::Webhook(_) => TriggerKindDto::Webhook,
        TriggerKind::Flow(_) => TriggerKindDto::Flow,
        TriggerKind::Leaf { .. } => TriggerKindDto::Other,
    };
    let yaml_disabled = parsed.disabled || t.disabled;
    let switch = state.switches.get(&key);
    let runtime_disabled = switch.is_some();
    let effective_disabled =
        crate::scheduler::effective_disabled(parsed.disabled, t.disabled, runtime_disabled)
            || flow.deleted
            || flow.state == "disabled";
    // First match wins (§3.5).
    let toggle_refusal = if flow.deleted {
        Some(ToggleRefusal::FlowDeleted)
    } else if kind == TriggerKindDto::Other {
        Some(ToggleRefusal::NotSupported)
    } else if !may_manage {
        Some(ToggleRefusal::Forbidden)
    } else if yaml_disabled {
        Some(ToggleRefusal::YamlDisabled)
    } else {
        None
    };
    let duplicate_id = parsed.triggers.iter().filter(|o| o.id == t.id).count() > 1;

    let schedule = match &t.kind {
        TriggerKind::Schedule(s) => {
            let row = state.schedules.get(&key);
            let next = row.and_then(|r| r.next_evaluation_date);
            Some(ScheduleInfoDto {
                cron: s.cron.clone(),
                with_seconds: s.with_seconds,
                timezone: s.timezone.clone().unwrap_or_else(|| "UTC".to_string()),
                next_evaluation_date: next,
                evaluated_at: row.and_then(|r| r.evaluated_at),
                last_error: row.and_then(|r| r.last_error.clone()),
                off_queue: next.is_none(),
                missed_ticks: match detail {
                    Detail::Full => missed_ticks(t, next, state.db_now),
                    Detail::Row => None,
                },
                recover_missed_schedules: s.recover_missed_schedules.as_ref().map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                }),
            })
        }
        _ => None,
    };

    let flow_info = match &t.kind {
        TriggerKind::Flow(spec) => {
            let not_listening_reason = if spec.depends_on.is_empty() {
                WindowFreeFlowTrigger::resolve(t)
                    .err()
                    .map(|e| e.to_string())
            } else {
                None
            };
            // A window whose end has passed is none: the sweep or the next
            // event removes it, and nothing it holds is met any more.
            let window_row = state
                .windows
                .get(&key)
                .filter(|w| w.window_end > state.db_now);
            let depends_on = (!spec.depends_on.is_empty()).then(|| DependsOnDto {
                mode: wire_name(&spec.mode),
                min_satisfied: spec.min_satisfied,
                conditions: match detail {
                    Detail::Full => Some(
                        spec.depends_on
                            .iter()
                            .map(|dep| {
                                let k = condition_key(dep);
                                ConditionDto {
                                    namespace: dep.namespace.clone(),
                                    flow_id: dep.flow_id.clone(),
                                    states: dep
                                        .states
                                        .as_ref()
                                        .map(|s| s.iter().map(state_name).collect()),
                                    labels: dep.labels.as_ref().map(|l| {
                                        serde_json::Value::Object(
                                            l.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                                        )
                                    }),
                                    when: dep.when.as_ref().map(|w| w.source().to_string()),
                                    met: window_row.is_some_and(|w| {
                                        w.results.get(&k) == Some(&serde_json::Value::Bool(true))
                                    }),
                                }
                            })
                            .collect(),
                    ),
                    Detail::Row => None,
                },
                window: window_row.and_then(|w| {
                    let kind = match TimeWindow::from_model(spec.window.as_ref()).ok()?.window {
                        TimeWindow::Duration { .. } => "duration",
                        TimeWindow::Sliding { .. } => "sliding",
                        TimeWindow::DailyWindow { .. } => "daily_window",
                        TimeWindow::DailyDeadline { .. } => "daily_deadline",
                    };
                    Some(WindowDto {
                        kind: kind.to_string(),
                        start: w.window_start,
                        end: w.window_end,
                    })
                }),
            });
            Some(FlowTriggerInfoDto {
                states: spec.states.iter().map(state_name).collect(),
                listening: state.listeners.contains_key(&key),
                not_listening_reason,
                depends_on,
            })
        }
        _ => None,
    };

    let webhook = match (&t.kind, detail) {
        (TriggerKind::Webhook(w), Detail::Full) => Some(webhook_info(flow, parsed, t, w)),
        _ => None,
    };

    TriggerEntryDto {
        trigger_id: t.id.clone(),
        r#type: t.r#type.clone(),
        kind,
        yaml_disabled,
        runtime_disabled,
        disabled_by: switch.and_then(|s| s.disabled_by),
        disabled_by_name: switch.and_then(|s| s.disabled_by_name.clone()),
        disabled_at: switch.map(|s| s.disabled_at),
        effective_disabled,
        can_toggle: toggle_refusal.is_none(),
        toggle_refusal,
        duplicate_id,
        schedule,
        flow: flow_info,
        webhook,
        last_execution: None,
    }
}

/// The ticks of a schedule due and not fired: on the queue (`next` is set)
/// and `next <= now`, the ticks from `next` through `now`, up to
/// [`MISSED_TICKS_CAP`]. A switched-off schedule keeps its date, so it is
/// overdue by the same rule. `None` off the queue, not yet due, or when the
/// schedule does not resolve.
pub(crate) fn missed_ticks(
    t: &Trigger,
    next: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<MissedTicksDto> {
    let next = next.filter(|n| *n <= now)?;
    let (count, more) =
        ScheduleTrigger::resolve(t)
            .ok()?
            .ticks_through(next, now, MISSED_TICKS_CAP);
    Some(MissedTicksDto { count, more })
}

/// The webhook's path, built as the webhook route matches it: the key
/// rendered against the context `webhook_routes::handle` builds (the parsed
/// flow's id, namespace and variables), the namespace and flow id from the
/// row's columns, each segment percent-encoded.
fn webhook_info(
    flow: &FlowCtx,
    parsed: &Flow,
    t: &Trigger,
    w: &crate::models::Webhook,
) -> WebhookInfoDto {
    let kcx = KeyContext {
        flow_id: parsed.id.clone(),
        namespace: parsed.namespace.clone(),
        vars: parsed.variables.clone().into_iter().collect(),
    };
    let (url_path, key_error) = if w.key.is_none() {
        (None, Some("no key is declared".to_string()))
    } else {
        match crate::trigger::webhook::render_key(t, w, &kcx) {
            Some(key) => (
                Some(format!(
                    "/workflows/v1/{}/executions/webhook/{}/{}/{}",
                    encode_segment(flow.ws_slug),
                    encode_segment(flow.namespace),
                    encode_segment(flow.flow_id),
                    encode_segment(&key),
                )),
                None,
            ),
            None => (None, Some("the key does not render".to_string())),
        }
    };
    WebhookInfoDto {
        url_path,
        key_error,
        unsupported: crate::trigger::webhook::declared_unsupported(w).map(|u| u.message(&t.id)),
    }
}

/// Percent-encodes every byte of `s` outside RFC 3986's unreserved set
/// (`A-Z a-z 0-9 - . _ ~`), so the segment cannot become a `/`, `?` or `#`.
pub(crate) fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A value's wire spelling (`SUCCESS`, `AT_LEAST`).
fn wire_name<T: serde::Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        // Every caller passes a unit enum, which serialises as a string; a
        // name is still better than a panic in a read route.
        Ok(other) => other.to_string(),
        Err(e) => format!("<unserialisable: {e}>"),
    }
}

fn state_name(s: &State) -> String {
    wire_name(s)
}

#[cfg(test)]
mod tests {
    use super::super::state::{ListenerRow, ScheduleRow, Switch, WindowRow};
    use super::*;
    use chrono::{Duration, TimeZone};

    const SCHEDULE: &str = "io.kestra.plugin.core.trigger.Schedule";
    const WEBHOOK: &str = "io.kestra.plugin.core.trigger.Webhook";
    const FLOW: &str = "io.kestra.plugin.core.trigger.Flow";

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 30).unwrap()
    }

    fn parse(triggers: &str) -> Flow {
        crate::spec::parse(&format!(
            "id: f\nnamespace: n.s\nvariables:\n  env: prod\ntriggers:\n{triggers}tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
        ))
        .expect("fixture parses")
    }

    fn ctx(id: Uuid, deleted: bool) -> FlowCtx<'static> {
        FlowCtx {
            id,
            ws_slug: "my ws",
            namespace: "n.s",
            flow_id: "f",
            state: "draft",
            deleted,
        }
    }

    fn one(src: &str, state: &StateRows, may_manage: bool, deleted: bool) -> TriggerEntryDto {
        let id = Uuid::nil();
        let mut v = entries_for_flow(
            &ctx(id, deleted),
            &parse(src),
            state,
            may_manage,
            Detail::Full,
        );
        assert_eq!(v.len(), 1);
        v.remove(0)
    }

    /// `state = 'disabled'` fires nothing, so every entry is effective-off;
    /// neither the YAML nor the switch says so, and the switch stays usable.
    #[test]
    fn a_disabled_state_column_is_effective_disabled() {
        let flow = parse(&format!(
            "  - id: s\n    type: {SCHEDULE}\n    cron: \"*/1 * * * *\"\n"
        ));
        let mut c = ctx(Uuid::nil(), false);
        c.state = "disabled";
        let e = &entries_for_flow(&c, &flow, &StateRows::empty(now()), true, Detail::Full)[0];
        assert!(e.effective_disabled && !e.yaml_disabled && !e.runtime_disabled && e.can_toggle);
        c.state = "live";
        let e = &entries_for_flow(&c, &flow, &StateRows::empty(now()), true, Detail::Full)[0];
        assert!(!e.effective_disabled);
    }

    #[test]
    fn toggle_refusal_first_match_wins() {
        let st = StateRows::empty(now());
        let sched = format!(
            "  - id: s\n    type: {SCHEDULE}\n    cron: \"*/1 * * * *\"\n    disabled: true\n"
        );
        let leaf = "  - id: l\n    type: io.kestra.plugin.x.Poll\n".to_string();
        let on = format!("  - id: s\n    type: {SCHEDULE}\n    cron: \"*/1 * * * *\"\n");
        // Deleted beats everything.
        let e = one(&leaf, &st, false, true);
        assert_eq!(e.toggle_refusal, Some(ToggleRefusal::FlowDeleted));
        assert!(e.effective_disabled, "a trashed flow's trigger is off");
        // Leaf beats the permission.
        assert_eq!(
            one(&leaf, &st, false, false).toggle_refusal,
            Some(ToggleRefusal::NotSupported)
        );
        // The permission beats the YAML.
        assert_eq!(
            one(&sched, &st, false, false).toggle_refusal,
            Some(ToggleRefusal::Forbidden)
        );
        assert_eq!(
            one(&sched, &st, true, false).toggle_refusal,
            Some(ToggleRefusal::YamlDisabled)
        );
        let e = one(&on, &st, true, false);
        assert!(e.can_toggle && e.toggle_refusal.is_none() && !e.effective_disabled);
    }

    #[test]
    fn a_switch_row_is_runtime_and_effective() {
        let mut st = StateRows::empty(now());
        let by = Uuid::new_v4();
        st.switches.insert(
            (Uuid::nil(), "s".into()),
            Switch {
                flow_id: Uuid::nil(),
                trigger_id: "s".into(),
                disabled_by: Some(by),
                disabled_by_name: Some("Ann".into()),
                disabled_at: now(),
            },
        );
        let e = one(
            &format!("  - id: s\n    type: {SCHEDULE}\n    cron: \"*/1 * * * *\"\n"),
            &st,
            true,
            false,
        );
        assert!(e.runtime_disabled && e.effective_disabled && !e.yaml_disabled && e.can_toggle);
        assert_eq!(e.disabled_by, Some(by));
        assert_eq!(e.disabled_by_name.as_deref(), Some("Ann"));
        assert_eq!(e.disabled_at, Some(now()));
    }

    #[test]
    fn the_flow_yaml_disables_every_trigger() {
        let flow = crate::spec::parse(&format!(
            "id: f\nnamespace: n.s\ndisabled: true\ntriggers:\n  - id: s\n    type: {SCHEDULE}\n    cron: \"*/1 * * * *\"\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
        ))
        .unwrap();
        let e = &entries_for_flow(
            &ctx(Uuid::nil(), false),
            &flow,
            &StateRows::empty(now()),
            true,
            Detail::Full,
        )[0];
        assert!(e.yaml_disabled && e.effective_disabled);
        assert_eq!(e.toggle_refusal, Some(ToggleRefusal::YamlDisabled));
    }

    #[test]
    fn duplicate_ids_are_marked() {
        let src = format!(
            "  - id: d\n    type: {SCHEDULE}\n    cron: \"*/1 * * * *\"\n  - id: d\n    type: {WEBHOOK}\n    key: k\n  - id: u\n    type: {WEBHOOK}\n    key: k2\n"
        );
        let v = entries_for_flow(
            &ctx(Uuid::nil(), false),
            &parse(&src),
            &StateRows::empty(now()),
            true,
            Detail::Full,
        );
        assert_eq!(
            v.iter().map(|e| e.duplicate_id).collect::<Vec<_>>(),
            vec![true, true, false]
        );
    }

    #[test]
    fn missed_ticks_count_from_the_stored_date_and_cap() {
        let t = &parse(&format!(
            "  - id: s\n    type: {SCHEDULE}\n    cron: \"*/1 * * * *\"\n"
        ))
        .triggers[0];
        let minute = Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 0).unwrap();
        // 11:58, 11:59, 12:00 are due at 12:00:30.
        assert_eq!(
            missed_ticks(t, Some(minute - Duration::minutes(2)), now()),
            Some(MissedTicksDto {
                count: 3,
                more: false
            })
        );
        assert_eq!(
            missed_ticks(t, Some(minute - Duration::minutes(2000)), now()),
            Some(MissedTicksDto {
                count: 1000,
                more: true
            })
        );
        assert_eq!(
            missed_ticks(t, Some(minute + Duration::minutes(1)), now()),
            None,
            "not due"
        );
        assert_eq!(missed_ticks(t, None, now()), None, "off the queue");
    }

    #[test]
    fn schedule_fields_come_from_the_yaml_and_the_row() {
        let mut st = StateRows::empty(now());
        st.schedules.insert(
            (Uuid::nil(), "s".into()),
            ScheduleRow {
                flow_id: Uuid::nil(),
                trigger_id: "s".into(),
                next_evaluation_date: None,
                evaluated_at: Some(now()),
                last_error: Some("boom".into()),
            },
        );
        let e = one(
            &format!("  - id: s\n    type: {SCHEDULE}\n    cron: \"0 * * * *\"\n    recoverMissedSchedules: LAST\n"),
            &st,
            true,
            false,
        );
        let s = e.schedule.unwrap();
        assert_eq!(s.cron.as_deref(), Some("0 * * * *"));
        assert_eq!(s.timezone, "UTC");
        assert!(s.off_queue && s.missed_ticks.is_none());
        assert_eq!(s.evaluated_at, Some(now()));
        assert_eq!(s.last_error.as_deref(), Some("boom"));
        assert_eq!(s.recover_missed_schedules.as_deref(), Some("LAST"));
    }

    fn depends_on_src() -> String {
        format!(
            "  - id: dep\n    type: {FLOW}\n    dependsOn:\n      - namespace: n.s\n        flowId: a\n      - namespace: n.s\n        flowId: b\n"
        )
    }

    fn window_state(end: DateTime<Utc>, met_key: String) -> StateRows {
        let mut st = StateRows::empty(now());
        st.listeners.insert(
            (Uuid::nil(), "dep".into()),
            ListenerRow {
                flow_id: Uuid::nil(),
                trigger_id: "dep".into(),
            },
        );
        st.windows.insert(
            (Uuid::nil(), "dep".into()),
            WindowRow {
                flow_id: Uuid::nil(),
                trigger_id: "dep".into(),
                window_start: end - Duration::days(1),
                window_end: end,
                results: serde_json::json!({ met_key: true }),
            },
        );
        st
    }

    #[test]
    fn conditions_are_met_by_their_own_key_in_an_open_window() {
        let flow = parse(&depends_on_src());
        let TriggerKind::Flow(spec) = &flow.triggers[0].kind else {
            unreachable!()
        };
        let key_a = condition_key(&spec.depends_on[0]);
        let st = window_state(now() + Duration::hours(1), key_a);
        let e = one(&depends_on_src(), &st, true, false);
        let f = e.flow.unwrap();
        assert!(f.listening && f.not_listening_reason.is_none());
        let d = f.depends_on.unwrap();
        assert_eq!(d.mode, "ALL");
        let met: Vec<bool> = d.conditions.unwrap().iter().map(|c| c.met).collect();
        assert_eq!(met, vec![true, false]);
        let w = d.window.unwrap();
        assert_eq!(w.kind, "duration");
        assert_eq!(w.end, now() + Duration::hours(1));
    }

    #[test]
    fn an_ended_window_is_none_and_meets_nothing() {
        let flow = parse(&depends_on_src());
        let TriggerKind::Flow(spec) = &flow.triggers[0].kind else {
            unreachable!()
        };
        let st = window_state(now(), condition_key(&spec.depends_on[0]));
        let d = one(&depends_on_src(), &st, true, false)
            .flow
            .unwrap()
            .depends_on
            .unwrap();
        assert!(d.window.is_none());
        assert!(d.conditions.unwrap().iter().all(|c| !c.met));
    }

    #[test]
    fn rows_omit_conditions_webhook_and_missed_ticks() {
        let src = format!(
            "{}  - id: h\n    type: {WEBHOOK}\n    key: k\n",
            depends_on_src()
        );
        let v = entries_for_flow(
            &ctx(Uuid::nil(), false),
            &parse(&src),
            &StateRows::empty(now()),
            true,
            Detail::Row,
        );
        assert!(v[0]
            .flow
            .as_ref()
            .unwrap()
            .depends_on
            .as_ref()
            .unwrap()
            .conditions
            .is_none());
        assert!(v[1].webhook.is_none());
    }

    #[test]
    fn the_webhook_path_renders_the_key_and_encodes_each_segment() {
        let st = StateRows::empty(now());
        let e = one(&format!("  - id: h\n    type: {WEBHOOK}\n    key: \"k/{{{{ flow.id }}}}-{{{{ vars.env }}}}\"\n"), &st, true, false);
        let w = e.webhook.unwrap();
        assert_eq!(
            w.url_path.as_deref(),
            Some("/workflows/v1/my%20ws/executions/webhook/n.s/f/k%2Ff-prod")
        );
        assert!(w.key_error.is_none() && w.unsupported.is_none());

        let w = one(
            &format!("  - id: h\n    type: {WEBHOOK}\n"),
            &st,
            true,
            false,
        )
        .webhook
        .unwrap();
        assert_eq!(
            (w.url_path, w.key_error.as_deref()),
            (None, Some("no key is declared"))
        );
        let w = one(
            &format!("  - id: h\n    type: {WEBHOOK}\n    key: \"{{{{ nope.x }}}}\"\n"),
            &st,
            true,
            false,
        )
        .webhook
        .unwrap();
        assert_eq!(
            (w.url_path, w.key_error.as_deref()),
            (None, Some("the key does not render"))
        );
        let w = one(
            &format!("  - id: h\n    type: {WEBHOOK}\n    key: k\n    wait: true\n"),
            &st,
            true,
            false,
        )
        .webhook
        .unwrap();
        assert!(w.unsupported.unwrap().contains("wait"));
    }

    #[test]
    fn a_window_free_trigger_that_does_not_resolve_says_why() {
        let src = format!("  - id: w\n    type: {FLOW}\n    window:\n      every: PT1H\n");
        let f = one(&src, &StateRows::empty(now()), true, false)
            .flow
            .unwrap();
        assert!(!f.listening);
        assert!(f.not_listening_reason.is_some(), "{f:?}");
    }
}
