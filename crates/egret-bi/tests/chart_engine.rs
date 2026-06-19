//! Engine-gated chart-builder smoke tests against live Trino. Self-skip unless EGRET_TEST_TRINO_URL is set.

use egret_bi::chartquery::{build, QuerySpec};
use egret_bi::{DatasetColumn, Metric, SourceSpec};
use egret_database::model::{ConnOptions, Engine};
use egret_engine::connector::{build_connector, ConnSpec};
use egret_engine::model::Limits;
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
    }
}

fn orders() -> SourceSpec {
    SourceSpec::Physical {
        catalog: Some("tpch".into()),
        schema: "tiny".into(),
        table: "orders".into(),
    }
}

async fn run(spec: &ConnSpec, sql: &str) -> egret_engine::model::QueryResult {
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
        eprintln!("skip: no Trino");
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
        rollup: false,
    };
    let sql = build(spec.engine, &orders(), &cols, &metrics, &qs).unwrap();
    let r = run(&spec, &sql).await;
    // tpch orders has 5 distinct order priorities
    assert!(!r.rows.is_empty(), "expected rows, sql=\n{sql}");
}

#[tokio::test]
async fn rollup_chart_adds_grand_total_on_trino() {
    let Some(spec) = trino_spec() else {
        eprintln!("skip: no Trino");
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
        rollup: true,
    };
    let sql = build(spec.engine, &orders(), &cols, &metrics, &qs).unwrap();
    let r = run(&spec, &sql).await;
    let grp_idx = r
        .columns
        .iter()
        .position(|c| c.name == "__grp_0")
        .expect("__grp_0 column present");
    // ROLLUP over one dimension yields per-priority rows + exactly one grand-total row (__grp_0 = 1).
    let grand_totals = r
        .rows
        .iter()
        .filter(|row| row[grp_idx].as_i64() == Some(1))
        .count();
    assert_eq!(grand_totals, 1, "expected one grand-total row, sql=\n{sql}");
}
