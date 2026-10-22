//! Validation of a chart's stored `params`.
//!
//! `ch_create` / `ch_update` used to write whatever JSON arrived straight into
//! `analytics.charts.params`, so `{}`, a pre-A2 (v1) body or a `spec` with a string where an
//! array belongs were all storable. Nothing noticed until a browser opened the chart, which
//! put the failure on whoever opened it instead of whoever wrote it.
//!
//! This checks the *shape* only. Column and metric existence is deliberately not checked: a
//! dataset changes after a chart is saved, and the run path already reports unknown columns
//! against the dataset as it is now. Settings keys are equally deliberately open — A2 chose
//! flat settings so keys an older build doesn't know still survive a round-trip.

use crate::chartquery::QuerySpec;
use crate::error::BiError;

/// Chart params as stored from A2 onwards. Anything else is a client bug.
pub const PARAMS_VERSION: u64 = 2;

/// Viz types the app can render. `viz_type` is a free-text column, so without this
/// `"Bar"` or `"bar "` is storable and then silently renders as the default.
pub const VIZ_TYPES: &[&str] = &[
    "table",
    "pivot_table",
    "bar",
    "row",
    "line",
    "area",
    "scatter",
    "combo",
    "pie",
    "scalar",
    "progress",
    "gauge",
    "funnel",
    "waterfall",
    "smartscalar",
    "boxplot",
    "sankey",
    "object",
    "map",
];

/// Rejects params the frontend could not open, naming the field at fault.
pub fn validate(viz_type: &str, params: &serde_json::Value) -> Result<(), BiError> {
    if !VIZ_TYPES.contains(&viz_type) {
        return Err(BiError::BadRequest(format!(
            "unknown viz_type {viz_type:?}"
        )));
    }

    let obj = params
        .as_object()
        .ok_or_else(|| BiError::BadRequest("params must be an object".into()))?;

    match obj.get("version").and_then(serde_json::Value::as_u64) {
        Some(v) if v == PARAMS_VERSION => {}
        Some(v) => {
            return Err(BiError::BadRequest(format!(
                "params.version must be {PARAMS_VERSION}, got {v}"
            )))
        }
        // A pre-A2 body has no `version` at all — the shape this app itself wrote before
        // migration 0099, so say that rather than "missing field".
        None => {
            return Err(BiError::BadRequest(format!(
                "params.version must be {PARAMS_VERSION} (a params object without a version is the pre-A2 shape)"
            )))
        }
    }

    let spec = obj
        .get("spec")
        .ok_or_else(|| BiError::BadRequest("params.spec is required".into()))?;
    let spec: QuerySpec = serde_json::from_value(spec.clone())
        .map_err(|e| BiError::BadRequest(format!("params.spec is not a valid query spec: {e}")))?;
    if spec.row_limit < 1 {
        return Err(BiError::BadRequest(format!(
            "params.spec.row_limit must be at least 1, got {}",
            spec.row_limit
        )));
    }

    match obj.get("settings") {
        Some(v) if v.is_object() => {}
        Some(_) => {
            return Err(BiError::BadRequest(
                "params.settings must be an object".into(),
            ))
        }
        None => return Err(BiError::BadRequest("params.settings is required".into())),
    }

    Ok(())
}

/// Rewrite a renamed metric through a chart's stored params, so the chart keeps running.
///
/// The query names the metric in `spec.metrics` and, when it sorts by it, in
/// `spec.orderby[].field`. The settings name it in a dozen viz-specific places (`cartesian.yMetrics`,
/// `table.metrics`, `pie.metric`, per-series maps keyed by the name, column formats keyed by the
/// name, conditional-formatting rules…), and their keys are open by design, so they are rewritten
/// by VALUE: every string equal to the old name and every object key equal to it. That is only
/// unambiguous while no column has the same name, which the caller checks before calling this.
///
/// Returns whether anything changed.
pub fn rename_metric(params: &mut serde_json::Value, old: &str, new: &str) -> bool {
    let mut changed = false;
    if let Some(spec) = params.get_mut("spec") {
        if let Some(ms) = spec.get_mut("metrics").and_then(|v| v.as_array_mut()) {
            for m in ms.iter_mut() {
                if m.as_str() == Some(old) {
                    *m = serde_json::Value::String(new.to_string());
                    changed = true;
                }
            }
        }
        if let Some(os) = spec.get_mut("orderby").and_then(|v| v.as_array_mut()) {
            for o in os.iter_mut() {
                if let Some(f) = o.get_mut("field") {
                    if f.as_str() == Some(old) {
                        *f = serde_json::Value::String(new.to_string());
                        changed = true;
                    }
                }
            }
        }
    }
    if let Some(settings) = params.get_mut("settings") {
        changed |= rename_in_settings(settings, old, new);
    }
    changed
}

/// The settings half of [`rename_metric`], also used for a dashboard card's `vizOverride`:
/// every string value and every object key equal to `old` becomes `new`, at any depth.
pub fn rename_in_settings(v: &mut serde_json::Value, old: &str, new: &str) -> bool {
    use serde_json::Value;
    match v {
        Value::String(s) if s == old => {
            *s = new.to_string();
            true
        }
        Value::Array(xs) => xs
            .iter_mut()
            .fold(false, |acc, x| rename_in_settings(x, old, new) | acc),
        Value::Object(map) => {
            let mut changed = false;
            if map.contains_key(old) {
                // Rebuilt rather than remove + insert: the map keeps insertion order
                // (`preserve_order`), and a rename should be the only difference in the row.
                for (k, x) in std::mem::take(map) {
                    let k = if k == old { new.to_string() } else { k };
                    map.insert(k, x);
                }
                changed = true;
            }
            for x in map.values_mut() {
                changed |= rename_in_settings(x, old, new);
            }
            changed
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn v2() -> serde_json::Value {
        json!({
            "version": 2,
            "spec": {
                "metrics": ["total_revenue"], "dimensions": ["region"], "columns": [],
                "filters": [], "orderby": [], "row_limit": 1000, "rollup_groups": []
            },
            "settings": { "cartesian.xAxis": "region", "cartesian.yMetrics": ["total_revenue"] }
        })
    }

    fn err(viz: &str, p: &serde_json::Value) -> String {
        match validate(viz, p) {
            Err(BiError::BadRequest(m)) => m,
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn accepts_what_the_frontend_writes() {
        validate("bar", &v2()).unwrap();
    }

    #[test]
    fn accepts_settings_keys_this_build_does_not_know() {
        // A2 carries unknown keys forward on purpose; rejecting them would break clients
        // newer than this server.
        let mut p = v2();
        p["settings"]["future.someSetting"] = json!({ "nested": true });
        validate("bar", &p).unwrap();
    }

    #[test]
    fn rejects_the_pre_a2_shape() {
        let v1 = json!({
            "spec": { "metrics": [], "dimensions": [], "columns": [], "filters": [],
                      "orderby": [], "row_limit": 100, "rollup_groups": [] },
            "cartesian": { "yMetrics": ["rev"] }
        });
        assert!(err("bar", &v1).contains("version"));
    }

    #[test]
    fn rejects_a_future_version() {
        let mut p = v2();
        p["version"] = json!(99);
        assert!(err("bar", &p).contains("got 99"));
    }

    #[test]
    fn rejects_a_missing_or_malformed_spec() {
        let mut p = v2();
        p.as_object_mut().unwrap().remove("spec");
        assert!(err("bar", &p).contains("params.spec is required"));

        let mut p = v2();
        p["spec"]["metrics"] = json!("total_revenue"); // a string where an array belongs
        assert!(err("bar", &p).contains("not a valid query spec"));
    }

    #[test]
    fn rejects_a_row_limit_the_query_would_never_use() {
        let mut p = v2();
        p["spec"]["row_limit"] = json!(-5);
        assert!(err("bar", &p).contains("row_limit"));
    }

    #[test]
    fn rejects_settings_that_are_not_an_object() {
        let mut p = v2();
        p["settings"] = json!([]);
        assert!(err("bar", &p).contains("settings must be an object"));

        let mut p = v2();
        p.as_object_mut().unwrap().remove("settings");
        assert!(err("bar", &p).contains("settings is required"));
    }

    #[test]
    fn rejects_params_that_are_not_an_object() {
        assert!(err("bar", &json!("nope")).contains("params must be an object"));
    }

    #[test]
    fn rejects_a_viz_type_that_only_looks_right() {
        assert!(err("Bar", &v2()).contains("unknown viz_type"));
        assert!(err("bar ", &v2()).contains("unknown viz_type"));
    }

    #[test]
    fn every_known_viz_type_is_accepted() {
        for viz in VIZ_TYPES {
            validate(viz, &v2()).unwrap_or_else(|e| panic!("{viz} rejected: {e}"));
        }
    }

    #[test]
    fn rename_metric_rewrites_the_query_and_every_settings_reference() {
        let mut p = json!({
            "version": 2,
            "spec": {
                "metrics": ["revenue", "orders"], "dimensions": ["region"], "columns": [],
                "filters": [], "orderby": [{"field": "revenue", "desc": true}, {"field": "region"}],
                "row_limit": 10, "rollup_groups": []
            },
            "settings": {
                "cartesian.xAxis": "region",
                "cartesian.yMetrics": ["revenue", "orders"],
                "cartesian.seriesSettings": {"revenue": {"color": "#f00"}, "orders": {}},
                "column_settings": {"region": {}, "revenue": {"decimals": 2}},
                "note": "revenue by region"
            }
        });
        assert!(rename_metric(&mut p, "revenue", "sales"));
        assert_eq!(p["spec"]["metrics"], json!(["sales", "orders"]));
        assert_eq!(p["spec"]["orderby"][0]["field"], "sales");
        assert_eq!(p["spec"]["orderby"][1]["field"], "region");
        assert_eq!(
            p["settings"]["cartesian.yMetrics"],
            json!(["sales", "orders"])
        );
        assert_eq!(p["settings"]["cartesian.xAxis"], "region");
        assert_eq!(
            p["settings"]["cartesian.seriesSettings"],
            json!({"sales": {"color": "#f00"}, "orders": {}})
        );
        // Key order is kept: the renamed key stays where it was.
        let keys: Vec<&String> = p["settings"]["column_settings"]
            .as_object()
            .unwrap()
            .keys()
            .collect();
        assert_eq!(keys, ["region", "sales"]);
        // A string that merely CONTAINS the name is not a reference to it.
        assert_eq!(p["settings"]["note"], "revenue by region");
        assert!(validate("bar", &p).is_ok(), "still a valid chart");
    }

    #[test]
    fn rename_metric_leaves_a_chart_that_does_not_use_it_alone() {
        let mut p = v2();
        let before = p.clone();
        assert!(!rename_metric(&mut p, "unused", "x"));
        assert_eq!(p, before);
    }
}
