//! Execution labels for a backfill (W7f spec §3.4): Kestra's merge and the
//! checks a backfill's own labels pass before they are stored.

// W7f Task 1 lands the model before its callers (Tasks 4-7); drop this then.
#![allow(dead_code)]

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
    for Label { key, value } in labels {
        if key.is_empty() {
            return Some("a label key is empty".to_string());
        }
        if key.starts_with("system.") {
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
    fn labels_at_the_caps_pass() {
        let labels: Vec<Label> = (0..MAX_BACKFILL_LABELS)
            .map(|i| {
                let key = format!("{i:0<width$}", width = MAX_LABEL_KEY_CHARS);
                l(&key, &"v".repeat(MAX_LABEL_VALUE_CHARS))
            })
            .collect();
        assert_eq!(backfill_labels_refusal(&labels), None);
    }
}
