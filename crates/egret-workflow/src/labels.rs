//! Execution labels (W7f spec §3.4; package 7d D5, D6): Kestra's merge, the
//! visible list, and the checks a backfill's and an edit's labels pass before
//! they are stored.

use crate::models::Label;

pub(crate) const MAX_BACKFILL_LABELS: usize = 50;
pub(crate) const MAX_LABEL_KEY_CHARS: usize = 128;
pub(crate) const MAX_LABEL_VALUE_CHARS: usize = 1024;

/// Kestra's `Execution.newExecution` + `Label.deduplicate`: `flow` then `own`,
/// an empty key or value dropped, the LAST value kept in the FIRST key's position.
pub(crate) fn execution_labels(flow: &[Label], own: &[Label]) -> Vec<Label> {
    let mut out: Vec<Label> = Vec::new();
    for label in flow.iter().chain(own) {
        if label.key.is_empty() || label.value.is_empty() {
            continue;
        }
        match out.iter_mut().find(|l| l.key == label.key) {
            Some(existing) => existing.value.clone_from(&label.value),
            None => out.push(label.clone()),
        }
    }
    out
}

/// `workflow.executions.labels` read back: NULL or a malformed item is no label.
pub(crate) fn own_labels(v: Option<&serde_json::Value>) -> Vec<Label> {
    let Some(serde_json::Value::Array(items)) = v else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| serde_json::from_value::<Label>(item.clone()).ok())
        .collect()
}

/// §3.4's label rules; `Some(sentence naming the label)` on the first breach.
pub(crate) fn backfill_labels_refusal(labels: &[Label]) -> Option<String> {
    if labels.len() > MAX_BACKFILL_LABELS {
        return Some(format!(
            "a backfill takes at most {MAX_BACKFILL_LABELS} labels, got {}",
            labels.len()
        ));
    }
    for (i, Label { key, value }) in labels.iter().enumerate() {
        if key.trim().is_empty() {
            return Some("a label key is empty or only whitespace".to_string());
        }
        if labels[..i].iter().any(|l| l.key == *key) {
            return Some(format!("label `{key}` is given more than once"));
        }
        if key.to_lowercase().starts_with("system.") {
            return Some(format!(
                "label `{key}` is reserved: keys starting with `system.` belong to the engine"
            ));
        }
        if value.is_empty() {
            return Some(format!("label `{key}` has an empty value"));
        }
        if key.chars().count() > MAX_LABEL_KEY_CHARS {
            return Some(format!(
                "label key `{key}` is longer than {MAX_LABEL_KEY_CHARS} characters"
            ));
        }
        if value.chars().count() > MAX_LABEL_VALUE_CHARS {
            return Some(format!(
                "the value of label `{key}` is longer than {MAX_LABEL_VALUE_CHARS} characters"
            ));
        }
        if value.contains("{{") || value.contains("{%") {
            return Some(format!(
                "the value of label `{key}` contains template syntax, which a backfill cannot render"
            ));
        }
    }
    None
}

/// The prefix of the labels only the engine writes (`system.restarted`,
/// `system.replay`; Kestra `Label.SYSTEM_PREFIX`). Case-sensitive, as
/// Kestra's `startsWith`.
#[allow(dead_code)] // read by package 7d T2 (readers) and T5 (set-labels)
pub(crate) const SYSTEM_LABEL_PREFIX: &str = "system.";

/// Kestra's label key pattern (`Label.java`, `@Pattern`): a lower-case
/// letter, then letters, digits, `.`, `_` and `-`.
#[allow(dead_code)] // read by package 7d T2 (readers) and T5 (set-labels)
static LABEL_KEY: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^\p{Ll}[\p{L}0-9._-]*$").expect("the label key pattern compiles")
});

/// **The labels an execution shows and renders** (package 7d D6). Every
/// reader of `workflow.executions.labels` calls this with
/// `labels_replaced` beside it. Not replaced: the flow's `labels:` then
/// the execution's own ([`execution_labels`]). Replaced (a person set the
/// whole list): the execution's own only, deduplicated the same way, so a
/// flow label that was removed stays removed.
pub(crate) fn visible_labels(flow: &[Label], own: &[Label], replaced: bool) -> Vec<Label> {
    if replaced {
        execution_labels(&[], own)
    } else {
        execution_labels(flow, own)
    }
}

/// **D5's rules for a replacement list**, checked against the execution's
/// visible list; `Some(sentence naming the label)` on the first breach.
/// The backfill's rules without its template rule (a backfill renders
/// nothing, an edit renders nothing either, but `{{` is a legal value
/// here as in Kestra), plus:
/// - Kestra's key pattern, **only for a key the visible list does not
///   already hold**: a flow may carry older keys that do not match, and
///   sending the list back unchanged must not fail;
/// - a `system.*` label passes only if the same key AND value are visible
///   (Kestra: "System labels can only be set by Kestra itself").
#[allow(dead_code)] // read by package 7d T2 (readers) and T5 (set-labels)
pub(crate) fn edit_labels_refusal(body: &[Label], visible: &[Label]) -> Option<String> {
    if body.len() > MAX_BACKFILL_LABELS {
        return Some(format!(
            "an execution takes at most {MAX_BACKFILL_LABELS} labels, got {}",
            body.len()
        ));
    }
    for (i, Label { key, value }) in body.iter().enumerate() {
        if key.trim().is_empty() {
            return Some("a label key is empty or only whitespace".to_string());
        }
        if body[..i].iter().any(|l| l.key == *key) {
            return Some(format!("label `{key}` is given more than once"));
        }
        if value.is_empty() {
            return Some(format!("label `{key}` has an empty value"));
        }
        if key.chars().count() > MAX_LABEL_KEY_CHARS {
            return Some(format!(
                "label key `{key}` is longer than {MAX_LABEL_KEY_CHARS} characters"
            ));
        }
        if value.chars().count() > MAX_LABEL_VALUE_CHARS {
            return Some(format!(
                "the value of label `{key}` is longer than {MAX_LABEL_VALUE_CHARS} characters"
            ));
        }
        if key.starts_with(SYSTEM_LABEL_PREFIX) {
            if !visible.iter().any(|l| l.key == *key && l.value == *value) {
                return Some(format!(
                    "label `{key}` is a system label: only the engine sets `system.*` labels, \
                     and one the execution already has can only be sent back unchanged"
                ));
            }
            continue;
        }
        if !visible.iter().any(|l| l.key == *key) && !LABEL_KEY.is_match(key) {
            return Some(format!(
                "label key `{key}` must start with a lower-case letter and hold only \
                 letters, digits, `.`, `_` and `-`"
            ));
        }
    }
    None
}

/// **Kestra `mergeSystemLabels`** (`EC:2383-2402`, package 7d D5): the
/// body, then each `system.*` label of `visible` whose key the body does
/// not hold, in `visible`'s order. Call after [`edit_labels_refusal`],
/// which has already refused a `system.*` label the body changed.
#[allow(dead_code)] // read by package 7d T2 (readers) and T5 (set-labels)
pub(crate) fn merge_system_labels(body: &[Label], visible: &[Label]) -> Vec<Label> {
    let mut out = body.to_vec();
    for l in visible
        .iter()
        .filter(|l| l.key.starts_with(SYSTEM_LABEL_PREFIX))
    {
        if !out.iter().any(|o| o.key == l.key) {
            out.push(l.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn l(k: &str, v: &str) -> Label {
        Label {
            key: k.into(),
            value: v.into(),
        }
    }

    #[test]
    fn execution_labels_keep_first_position_and_last_value() {
        let flow = [l("team", "data"), l("env", "prod")];
        let own = [l("team", "ops"), l("reason", "x")];
        assert_eq!(
            execution_labels(&flow, &own),
            vec![l("team", "ops"), l("env", "prod"), l("reason", "x")]
        );
    }

    #[test]
    fn execution_labels_drop_empty_keys_and_values() {
        let flow = [l("", "v"), l("a", ""), l("b", "1")];
        let own = [l("", "w"), l("c", ""), l("d", "2")];
        assert_eq!(
            execution_labels(&flow, &own),
            vec![l("b", "1"), l("d", "2")]
        );
    }

    #[test]
    fn no_own_labels_leave_the_flows_list_unchanged() {
        let flow = [l("team", "data"), l("env", "prod")];
        assert_eq!(execution_labels(&flow, &[]), flow.to_vec());
    }

    #[test]
    fn own_labels_reads_back_leniently() {
        assert_eq!(own_labels(None), vec![]);
        assert_eq!(own_labels(Some(&json!(null))), vec![]);
        assert_eq!(
            own_labels(Some(&json!([{"key":"a","value":"b"}, 5, {"key":"c"}]))),
            vec![l("a", "b")]
        );
    }

    #[test]
    fn backfill_labels_refusals_name_the_label() {
        let cases: Vec<(Vec<Label>, &str)> = vec![
            (vec![l("system.x", "v")], "system.x"),
            (vec![l("", "v")], "empty"),
            (vec![l("  ", "v")], "whitespace"),
            (vec![l("System.Y", "v")], "System.Y"),
            (vec![l("dup", "1"), l("dup", "2")], "dup"),
            (vec![l("k1", "")], "k1"),
            (vec![l("k2", "a {{ b }}")], "k2"),
            (vec![l("k3", "a {% b %}")], "k3"),
            ((0..51).map(|i| l(&format!("k{i}"), "v")).collect(), "50"),
            (vec![l(&"k".repeat(129), "v")], "128"),
            (vec![l("k4", &"v".repeat(1025))], "k4"),
        ];
        for (labels, needle) in cases {
            let r = backfill_labels_refusal(&labels);
            let msg = r.unwrap_or_else(|| panic!("no refusal for {labels:?}"));
            assert!(msg.contains(needle), "{msg:?} should mention {needle:?}");
        }
    }

    #[test]
    fn an_empty_own_value_cannot_erase_a_flow_label() {
        let flow = [l("team", "data")];
        let own = [l("team", "")];
        assert_eq!(execution_labels(&flow, &own), vec![l("team", "data")]);
    }

    /// The caps count Unicode scalar values (the spec says characters), not bytes.
    #[test]
    fn the_caps_count_characters_not_bytes() {
        let key_ok = "\u{e9}".repeat(MAX_LABEL_KEY_CHARS);
        let key_over = "\u{e9}".repeat(MAX_LABEL_KEY_CHARS + 1);
        let val_ok = "\u{e9}".repeat(MAX_LABEL_VALUE_CHARS);
        let val_over = "\u{e9}".repeat(MAX_LABEL_VALUE_CHARS + 1);
        assert_eq!(backfill_labels_refusal(&[l(&key_ok, &val_ok)]), None);
        assert!(backfill_labels_refusal(&[l(&key_over, "v")]).is_some());
        assert!(backfill_labels_refusal(&[l("k", &val_over)]).is_some());
    }

    #[test]
    fn labels_at_the_caps_pass() {
        let labels: Vec<Label> = (0..MAX_BACKFILL_LABELS)
            .map(|i| {
                let key = format!("{i:k<width$}", width = MAX_LABEL_KEY_CHARS);
                l(&key, &"v".repeat(MAX_LABEL_VALUE_CHARS))
            })
            .collect();
        assert_eq!(backfill_labels_refusal(&labels), None);
    }

    #[test]
    fn visible_labels_merge_the_flow_until_replaced() {
        let flow = [l("team", "data"), l("env", "prod")];
        let own = [l("env", "dev"), l("reason", "x")];
        assert_eq!(
            visible_labels(&flow, &own, false),
            vec![l("team", "data"), l("env", "dev"), l("reason", "x")]
        );
        assert_eq!(
            visible_labels(&flow, &own, true),
            vec![l("env", "dev"), l("reason", "x")],
            "replaced: the flow's `team` is gone"
        );
    }

    #[test]
    fn a_replaced_list_is_deduplicated_like_the_merge() {
        let own = [l("a", "1"), l("", "v"), l("b", ""), l("a", "2")];
        assert_eq!(
            visible_labels(&[l("x", "y")], &own, true),
            vec![l("a", "2")]
        );
        assert_eq!(visible_labels(&[l("x", "y")], &[], true), vec![]);
    }

    #[test]
    fn an_edit_may_resend_the_visible_list_unchanged() {
        // `Team` and `9lives` do not match Kestra's pattern, but the flow has them.
        let visible = [
            l("Team", "data"),
            l("9lives", "yes"),
            l("system.restarted", "true"),
        ];
        assert_eq!(edit_labels_refusal(&visible, &visible), None);
        assert_eq!(
            edit_labels_refusal(&[], &visible),
            None,
            "an empty list is a valid edit"
        );
    }

    #[test]
    fn edit_refusals_name_the_label() {
        let visible = [l("team", "data"), l("system.restarted", "true")];
        let cases: Vec<(Vec<Label>, &str)> = vec![
            (vec![l("", "v")], "empty"),
            (vec![l("  ", "v")], "whitespace"),
            (vec![l("dup", "1"), l("dup", "2")], "dup"),
            (vec![l("k1", "")], "k1"),
            (vec![l("Upper", "v")], "Upper"),
            (vec![l("1st", "v")], "1st"),
            (vec![l("sp ace", "v")], "sp ace"),
            (vec![l("system.restarted", "false")], "system.restarted"),
            (vec![l("system.replay", "true")], "system.replay"),
            ((0..51).map(|i| l(&format!("k{i}"), "v")).collect(), "50"),
            (vec![l(&format!("k{}", "a".repeat(128)), "v")], "128"),
            (vec![l("k4", &"v".repeat(1025))], "k4"),
        ];
        for (body, needle) in cases {
            let msg = edit_labels_refusal(&body, &visible)
                .unwrap_or_else(|| panic!("no refusal for {body:?}"));
            assert!(msg.contains(needle), "{msg:?} should mention {needle:?}");
        }
    }

    #[test]
    fn kestra_keys_and_template_values_pass() {
        let body = [
            l("team", "{{ not rendered }}"),
            l("é.x_y-1", "v"),
            l("a", " "),
        ];
        assert_eq!(edit_labels_refusal(&body, &[]), None);
    }

    #[test]
    fn an_unchanged_system_label_passes_and_is_kept_once() {
        let visible = [l("team", "data"), l("system.restarted", "true")];
        let body = [l("system.restarted", "true"), l("env", "dev")];
        assert_eq!(edit_labels_refusal(&body, &visible), None);
        assert_eq!(merge_system_labels(&body, &visible), body.to_vec());
    }

    #[test]
    fn merge_system_labels_appends_what_the_body_left_out() {
        let visible = [
            l("system.restarted", "true"),
            l("team", "data"),
            l("system.replay", "true"),
        ];
        assert_eq!(
            merge_system_labels(&[l("env", "dev")], &visible),
            vec![
                l("env", "dev"),
                l("system.restarted", "true"),
                l("system.replay", "true")
            ]
        );
        assert_eq!(
            merge_system_labels(&[], &visible),
            vec![l("system.restarted", "true"), l("system.replay", "true")]
        );
    }

    #[test]
    fn the_system_prefix_is_case_sensitive_as_kestras() {
        // `System.x` is not a system label; it fails the key pattern instead.
        let msg = edit_labels_refusal(&[l("System.x", "v")], &[]).expect("refused");
        assert!(msg.contains("lower-case"), "{msg}");
        assert_eq!(merge_system_labels(&[], &[l("System.x", "v")]), vec![]);
    }
}
