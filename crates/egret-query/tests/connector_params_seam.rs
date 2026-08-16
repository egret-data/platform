use egret_core::tabular::TabularResult;
use egret_query::execution::connector::Connector;
use egret_query_api::{Limits, Param};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Explicit, non-zero values on purpose: `Limits` deliberately does not derive
/// `Default` (see its doc comment) because `max_rows: 0` / `timeout: ZERO`
/// are dangerous defaults — a real caller must set both fields, so this test
/// does too rather than reaching for a shortcut that doesn't exist.
fn limits() -> Limits {
    Limits {
        max_rows: 10,
        timeout: Duration::from_secs(5),
    }
}

struct Legacy;

#[async_trait::async_trait]
impl Connector for Legacy {
    async fn execute(
        &self,
        _sql: &str,
        _l: &Limits,
        _c: CancellationToken,
    ) -> Result<TabularResult, egret_query::execution::EngineError> {
        Ok(TabularResult::default())
    }
}

#[tokio::test]
async fn a_connector_that_has_not_opted_in_refuses_params_instead_of_ignoring_them() {
    let err = Legacy
        .execute_with_params(
            "SELECT ?",
            &[Param::I64(1)],
            &limits(),
            CancellationToken::new(),
        )
        .await
        .expect_err(
            "a connector with no parameter support must refuse, not silently drop the values",
        );
    assert!(
        err.to_string()
            .contains("does not support bound parameters"),
        "got: {err}"
    );
}

#[tokio::test]
async fn no_params_still_delegates_to_the_plain_path() {
    Legacy
        .execute_with_params("SELECT 1", &[], &limits(), CancellationToken::new())
        .await
        .expect("an empty parameter list must behave exactly like execute()");
}
