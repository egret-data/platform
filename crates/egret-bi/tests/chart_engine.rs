//! Engine-gated chart-builder smoke tests against live Trino. Self-skip unless EGRET_TEST_TRINO_URL is set.

use egret_bi::chartquery::{build, QuerySpec};
use egret_bi::{DatasetColumn, Metric, SourceSpec};
use egret_query::execution::connector::{build_connector, ConnSpec};
use egret_query::execution::model::Limits;
use egret_query_api::{ConnOptions, Engine};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn trino_spec() -> Option<ConnSpec> {
    let base = std::env::var("EGRET_TEST_TRINO_URL").ok()?;
    let url = url::Url::parse(&base).ok()?;
    Some(ConnSpec {
        engine: Engine::Trino,
        host: url.host_str()?.to_string(),
        port: url.port().unwrap_or(8080) as i32,
        options: ConnOptions {
            user: Some("admin".into()),
            http_scheme: Some(url.scheme().to_string()),
            ..Default::default()
        },
        secret: None,
        session_user: None,
        bearer_token: None,
    })
}

fn dim(name: &str) -> DatasetColumn {
    DatasetColumn {
        column_name: name.into(),
        data_type: "varchar".into(),
        type_generic: "string".into(),
        is_temporal: false,
        ordinal: 0,
        expression: None,
        verbose_name: String::new(),
        description: String::new(),
        groupby: true,
        filterable: true,
        format: String::new(),
        semantic_type: String::new(),
    }
}

fn revenue() -> Metric {
    Metric {
        id: uuid::Uuid::nil(),
        metric_name: "revenue".into(),
        expression: "sum(totalprice)".into(),
        data_type: "double".into(),
        type_generic: "numeric".into(),
        verbose_name: String::new(),
        description: String::new(),
        ordinal: 0,
        format: String::new(),
        certified: false,
        certified_by: String::new(),
        certification_details: String::new(),
    }
}

fn orders() -> SourceSpec {
    SourceSpec::Physical {
        catalog: Some("tpch".into()),
        schema: "tiny".into(),
        table: "orders".into(),
    }
}

async fn run(spec: &ConnSpec, sql: &str) -> egret_core::TabularResult {
    build_connector(spec)
        .execute(
            sql,
            &Limits {
                max_rows: 1000,
                timeout: Duration::from_secs(30),
            },
            CancellationToken::new(),
        )
        .await
        .expect("query executes")
}

#[tokio::test]
async fn table_chart_runs_on_trino() {
    let Some(spec) = trino_spec() else {
        eprintln!("skip: EGRET_TEST_TRINO_URL not set");
        return;
    };
    let cols = vec![dim("orderpriority")];
    let metrics = vec![revenue()];
    let qs = QuerySpec {
        metrics: vec!["revenue".into()],
        dimensions: vec!["orderpriority".into()],
        columns: vec![],
        filters: vec![],
        orderby: vec![],
        row_limit: 100,
        rollup_groups: vec![],
        temporal_units: std::collections::HashMap::new(),
    };
    let sql = build(spec.engine, &orders(), &cols, &metrics, &qs).unwrap();
    let r = run(&spec, &sql).await;
    assert!(!r.truncated, "unexpected truncation");
    assert!(
        r.columns.iter().any(|c| c.name == "orderpriority"),
        "missing orderpriority column, cols={:?}",
        r.columns.iter().map(|c| &c.name).collect::<Vec<_>>()
    );
    assert!(
        r.columns.iter().any(|c| c.name == "revenue"),
        "missing revenue metric column, cols={:?}",
        r.columns.iter().map(|c| &c.name).collect::<Vec<_>>()
    );
    // tpch tiny.orders has exactly 5 distinct order priorities.
    assert_eq!(r.rows.len(), 5, "expected 5 priority rows, sql=\n{sql}");
}

#[tokio::test]
async fn pivot_rollup_x_rollup_runs_on_trino() {
    let Some(spec) = trino_spec() else {
        eprintln!("skip: EGRET_TEST_TRINO_URL not set");
        return;
    };
    let cols = vec![dim("orderpriority"), dim("orderstatus")];
    let metrics = vec![revenue()];
    let qs = QuerySpec {
        metrics: vec!["revenue".into()],
        dimensions: vec!["orderpriority".into(), "orderstatus".into()],
        columns: vec![],
        filters: vec![],
        orderby: vec![],
        row_limit: 1000,
        rollup_groups: vec![vec!["orderpriority".into()], vec!["orderstatus".into()]],
        temporal_units: std::collections::HashMap::new(),
    };
    let sql = build(spec.engine, &orders(), &cols, &metrics, &qs).unwrap();
    let r = run(&spec, &sql).await;
    assert!(!r.truncated, "unexpected truncation");
    let g0 = r
        .columns
        .iter()
        .position(|c| c.name == "__grp_0")
        .expect("__grp_0");
    let g1 = r
        .columns
        .iter()
        .position(|c| c.name == "__grp_1")
        .expect("__grp_1");
    // grand total row: both dims rolled up.
    let grand = r
        .rows
        .iter()
        .filter(|row| row[g0].as_i64() == Some(1) && row[g1].as_i64() == Some(1))
        .count();
    assert_eq!(grand, 1, "expected one grand-total row, sql=\n{sql}");
    // row subtotals (priority kept, status rolled up): g0=0, g1=1; tpch has 5 priorities.
    let row_subtotals = r
        .rows
        .iter()
        .filter(|row| row[g0].as_i64() == Some(0) && row[g1].as_i64() == Some(1))
        .count();
    assert!(
        row_subtotals >= 5,
        "expected per-priority subtotals, sql=\n{sql}"
    );
}

/// KI-100: a metric must be an aggregate. `totalprice` is a valid expression on its own and used
/// to be stored, then failed inside every chart with "must be an aggregate expression or appear
/// in GROUP BY clause". The aggregate probe refuses it on save; an aggregate and a constant pass.
#[tokio::test]
async fn a_non_aggregate_metric_is_refused_on_save() {
    let Some(spec) = trino_spec() else {
        eprintln!("skip: EGRET_TEST_TRINO_URL unset");
        return;
    };
    let err = egret_bi::validate_metric_expression(&spec, &orders(), "totalprice")
        .await
        .unwrap_err();
    match err {
        egret_bi::BiError::BadRequest(m) => {
            assert!(
                m.starts_with("metric expression must be an aggregate"),
                "{m}"
            )
        }
        other => panic!("expected a 400, got {other:?}"),
    }
    for ok in [
        "sum(totalprice)",
        "count(*)",
        "sum(totalprice) / count(*)",
        "1",
    ] {
        egret_bi::validate_metric_expression(&spec, &orders(), ok)
            .await
            .unwrap_or_else(|e| panic!("{ok}: {e:?}"));
    }
    // A typo is still the engine's own error, not the aggregate message.
    let err = egret_bi::validate_metric_expression(&spec, &orders(), "sum(nope)")
        .await
        .unwrap_err();
    assert!(
        !matches!(err, egret_bi::BiError::BadRequest(ref m) if m.contains("aggregate")),
        "{err:?}"
    );
}
