//! Engine-gated SchemaService test. Self-skip unless EGRET_TEST_TRINO_URL is set.
use egret_bi::SchemaService;
use egret_database::model::{ConnOptions, Engine};

fn trino_spec() -> Option<egret_engine::connector::ConnSpec> {
    let base = std::env::var("EGRET_TEST_TRINO_URL").ok()?;
    let u = url::Url::parse(&base).ok()?;
    Some(egret_engine::connector::ConnSpec {
        engine: Engine::Trino,
        host: u.host_str()?.to_string(),
        port: u.port().unwrap_or(8080) as i32,
        options: ConnOptions {
            user: Some("admin".into()),
            http_scheme: Some(u.scheme().to_string()),
            ..Default::default()
        },
        secret: None,
        session_user: None,
    })
}

#[tokio::test]
async fn trino_lists_catalogs_and_schemas() {
    let Some(spec) = trino_spec() else {
        eprintln!("skip");
        return;
    };
    let svc = SchemaService::new();
    let cats = svc.catalogs(&spec).await.unwrap();
    assert!(!cats.is_empty(), "expected at least one catalog");
    // every Trino has the `system` catalog
    assert!(cats.iter().any(|c| c == "system"), "catalogs: {cats:?}");
    let schemas = svc.schemas(&spec, "system").await.unwrap();
    assert!(!schemas.is_empty(), "expected schemas under system");
}
