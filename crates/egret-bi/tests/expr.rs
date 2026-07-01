//! Engine-gated expression-validation tests. Self-skip unless EGRET_TEST_TRINO_URL is set.

use egret_database::model::{ConnOptions, Engine};
use egret_engine::connector::ConnSpec;

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

#[tokio::test]
async fn valid_expression_infers_type() {
    let Some(spec) = trino_spec() else {
        eprintln!("skip: no Trino");
        return;
    };
    let src = egret_bi::SourceSpec::Physical {
        catalog: Some("tpch".into()),
        schema: "tiny".into(),
        table: "orders".into(),
    };
    let d = egret_bi::validate_expression(&spec, &src, "sum(totalprice)")
        .await
        .expect("valid");
    assert_eq!(d.type_generic, "numeric");
}

#[tokio::test]
async fn invalid_expression_errors() {
    let Some(spec) = trino_spec() else {
        eprintln!("skip: no Trino");
        return;
    };
    let src = egret_bi::SourceSpec::Physical {
        catalog: Some("tpch".into()),
        schema: "tiny".into(),
        table: "orders".into(),
    };
    assert!(
        egret_bi::validate_expression(&spec, &src, "no_such_fn(totalprice)")
            .await
            .is_err()
    );
}
