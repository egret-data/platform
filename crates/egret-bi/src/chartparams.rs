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
}
