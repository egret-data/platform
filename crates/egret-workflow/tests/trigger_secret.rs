//! Package 10a Task 4: `secret()` in a Webhook trigger's `inputs:` and a Flow
//! trigger's `when:` / `inputs:`, through the REAL route and the REAL
//! processor. Requires DATABASE_URL.
//!
//! Neither seam may let a value into a stored row: the input is stored sealed
//! (`secret_inputs`), a `when` is kept as its verdict, and a render error is
//! masked. The Webhook's `key:` -- a credential compared per request -- stays
//! refused.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::http::StatusCode;
use egret_workflow::flow_trigger::process_events;
use egret_workflow::runners::secrets::{self, SecretError, SecretSource};
use egret_workflow::worker::{tick, Handlers};
use uuid::Uuid;

const SENTINEL: &str = "trg-SENTINEL-3c";

/// The tests tick the shared queue and a flow-trigger pass is global, so one
/// runs at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Token;

#[async_trait::async_trait]
impl SecretSource for Token {
    async fn get(
        &self,
        _workspace: Uuid,
        name: &str,
    ) -> Result<BTreeMap<String, String>, SecretError> {
        if name == "TOKEN" {
            Ok(BTreeMap::from([(
                "value".to_string(),
                SENTINEL.to_string(),
            )]))
        } else {
            Err(SecretError::NotFound { name: name.into() })
        }
    }
}

async fn settle_all(pool: &sqlx::PgPool) {
    let reg = Handlers::with_core_tasks(None);
    for _ in 0..64 {
        if tick(pool, 16, &reg).await.expect("tick") == 0 {
            break;
        }
    }
}

/// How many stored rows of `executions` hold `SENTINEL`: the whole execution
/// row but its `source` (the author's own literal lives there), its task runs
/// and its logs.
async fn leaks(pool: &sqlx::PgPool, executions: &[Uuid]) -> i64 {
    sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM workflow.task_logs t \
                  WHERE t.execution_id = ANY($1) AND t::text LIKE '%' || $2 || '%') \
              + (SELECT count(*) FROM workflow.task_runs t \
                  WHERE t.execution_id = ANY($1) AND t::text LIKE '%' || $2 || '%') \
              + (SELECT count(*) FROM workflow.executions e \
                  WHERE e.id = ANY($1) AND (to_jsonb(e) - 'source')::text LIKE '%' || $2 || '%')",
    )
    .bind(executions)
    .bind(SENTINEL)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn state_of(pool: &sqlx::PgPool, ex: Uuid, task: &str) -> String {
    sqlx::query_scalar(
        "SELECT state FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(ex)
    .bind(task)
    .fetch_one(pool)
    .await
    .unwrap()
}

fn ns() -> String {
    format!("trg.{}", Uuid::new_v4().simple())
}

#[tokio::test]
async fn a_webhook_input_resolves_secret_sealed_and_never_stored_in_clear() {
    let _serial = SERIAL.lock().await;
    let Some(db) = common::db().await else { return };
    let _ = secrets::install(Arc::new(Token));
    egret_workflow::secret_inputs::install_for_tests();
    let (ws, user) = common::seed_workspace(&db).await;
    let ns = ns();
    let flow = |value: &str| {
        format!(
            "id: hook\nnamespace: {ns}\ninputs:\n  - id: x\n    type: STRING\n\
             tasks:\n  - id: real\n    type: io.kestra.plugin.core.log.Log\n    \
             message: \"got {{{{ inputs.x }}}}\"\n    runIf: \"{{{{ inputs.x == '{SENTINEL}' }}}}\"\n\
             triggers:\n  - id: w\n    type: io.kestra.plugin.core.trigger.Webhook\n    key: fx-key\n    \
             inputs:\n      x: \"{value}\"\n"
        )
    };
    common::seed_flow(&db, ws, user, &flow("{{ secret('TOKEN') }}"), "draft").await;
    let app = common::webhook_app(&db).await;
    let url = format!("/workflows/v1/{ws}/executions/webhook/{ns}/hook/fx-key");

    let res = common::anon(&app, "POST", &url, Some("application/json"), b"{}").await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&body).contains(SENTINEL),
        "the response does not echo the value"
    );
    let ex: Uuid = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    settle_all(&db.pool).await;

    let inputs: serde_json::Value =
        sqlx::query_scalar("SELECT inputs FROM workflow.executions WHERE id = $1")
            .bind(ex)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(inputs["x"], egret_workflow::secret_inputs::MASK, "sealed");
    assert_eq!(
        state_of(&db.pool, ex, "real").await,
        "SUCCESS",
        "the downstream render saw the REAL value"
    );
    assert_eq!(leaks(&db.pool, &[ex]).await, 0, "no stored row holds it");
}

#[tokio::test]
async fn a_webhook_input_naming_a_missing_secret_is_refused_without_a_value() {
    let _serial = SERIAL.lock().await;
    let Some(db) = common::db().await else { return };
    let _ = secrets::install(Arc::new(Token));
    egret_workflow::secret_inputs::install_for_tests();
    let (ws, user) = common::seed_workspace(&db).await;
    let ns = ns();
    let src = format!(
        "id: hook\nnamespace: {ns}\ninputs:\n  - id: x\n    type: STRING\n\
         tasks:\n  - id: real\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n\
         triggers:\n  - id: w\n    type: io.kestra.plugin.core.trigger.Webhook\n    key: fx-key\n    \
         inputs:\n      x: \"{{{{ secret('NOPE') }}}}\"\n"
    );
    common::seed_flow(&db, ws, user, &src, "draft").await;
    let app = common::webhook_app(&db).await;
    let url = format!("/workflows/v1/{ws}/executions/webhook/{ns}/hook/fx-key");
    let res = common::anon(&app, "POST", &url, Some("application/json"), b"{}").await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(common::execution_count(&db, ws).await, 0);
}

/// The control: a Webhook's `key:` is a credential compared per request and
/// stays refused, while the same trigger's `inputs:` resolve.
#[test]
fn a_webhook_key_still_refuses_secret_and_its_inputs_do_not() {
    let src = "id: f\nnamespace: a.b\ninputs:\n  - id: x\n    type: STRING\n\
               tasks:\n  - id: l\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n\
               triggers:\n  - id: w\n    type: io.kestra.plugin.core.trigger.Webhook\n    \
               key: \"{{ secret('K') }}\"\n    inputs:\n      x: \"{{ secret('TOKEN') }}\"\n";
    let flow = egret_workflow::spec::parse(src).expect("parses");
    let errors = egret_workflow::spec::validate(&flow).errors;
    let refused: Vec<&str> = errors
        .iter()
        .filter(|f| f.code == egret_workflow::plugins::CODE_SECRET_OUTSIDE_PLUGIN_TASK)
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(refused, vec!["triggers.w.key"], "{errors:?}");
}

#[tokio::test]
async fn a_flow_trigger_when_and_inputs_resolve_secret_and_no_row_holds_the_value() {
    let _serial = SERIAL.lock().await;
    let Some(db) = common::db().await else { return };
    let _ = secrets::install(Arc::new(Token));
    egret_workflow::secret_inputs::install_for_tests();
    let (ws, user) = common::seed_workspace(&db).await;
    let ns = ns();
    let listener = |id: &str, when: &str, input: &str| {
        format!(
            "id: {id}\nnamespace: {ns}\ninputs:\n  - id: x\n    type: STRING\n\
             tasks:\n  - id: real\n    type: io.kestra.plugin.core.log.Log\n    \
             message: \"got {{{{ inputs.x }}}}\"\n    runIf: \"{{{{ inputs.x == '{SENTINEL}' }}}}\"\n\
             triggers:\n  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    \
             states: [SUCCESS]\n    when: \"{when}\"\n    inputs:\n      x: \"{input}\"\n"
        )
    };
    let producer = format!(
        "id: producer\nnamespace: {ns}\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    );
    let on = "{{ flow.id == 'producer' and secret('TOKEN') == '".to_string() + SENTINEL + "' }}";
    let off = "{{ flow.id == 'producer' and secret('TOKEN') == 'other' }}";
    let fires = listener("fires", &on, "{{ secret('TOKEN') }}");
    let mismatch = listener("mismatch", off, "{{ secret('TOKEN') }}");
    let bad = listener(
        "bad",
        "{{ flow.id == 'producer' }}",
        "{{ secret('TOKEN') | number }}",
    );
    let fires_id = common::seed_flow(&db, ws, user, &fires, "draft").await;
    let mismatch_id = common::seed_flow(&db, ws, user, &mismatch, "draft").await;
    let bad_id = common::seed_flow(&db, ws, user, &bad, "draft").await;
    let producer_id = common::seed_flow(&db, ws, user, &producer, "draft").await;

    let src_ex = egret_workflow::worker::start_execution(
        &db.pool,
        ws,
        Some(producer_id),
        &producer,
        serde_json::json!({}),
    )
    .await
    .expect("start the producer");
    settle_all(&db.pool).await;
    let report = process_events(&db.pool).await.expect("a pass");
    assert_eq!(
        (report.fired, report.born_failed, report.refused),
        (1, 1, 1),
        "fires fired, bad born FAILED, mismatch refused: {report:?}"
    );
    settle_all(&db.pool).await;

    let of = |flow: Uuid| {
        let pool = db.pool.clone();
        async move {
            sqlx::query_as::<_, (Uuid, String, serde_json::Value)>(
                "SELECT id, state, inputs FROM workflow.executions WHERE flow_id = $1",
            )
            .bind(flow)
            .fetch_all(&pool)
            .await
            .unwrap()
        }
    };
    let fired = of(fires_id).await;
    assert_eq!(fired.len(), 1, "the matching `when` fired");
    let (fired_ex, _, inputs) = &fired[0];
    assert_eq!(inputs["x"], egret_workflow::secret_inputs::MASK, "sealed");
    assert_eq!(
        state_of(&db.pool, *fired_ex, "real").await,
        "SUCCESS",
        "the input held the REAL value downstream"
    );
    assert!(
        of(mismatch_id).await.is_empty(),
        "the `when` compared the real value, so a different literal does not fire"
    );
    let failed = of(bad_id).await;
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].1, "FAILED", "born FAILED, as Kestra does");

    let all: Vec<Uuid> = vec![*fired_ex, failed[0].0, src_ex];
    assert_eq!(leaks(&db.pool, &all).await, 0, "no stored row holds it");
}

/// Gap A: a Flow-trigger fire skips the start-time input check, so a key the
/// listener never declared reaches the store as rendered unless it is sealed.
#[tokio::test]
async fn an_undeclared_flow_trigger_input_from_secret_is_sealed_too() {
    let _serial = SERIAL.lock().await;
    let Some(db) = common::db().await else { return };
    let _ = secrets::install(Arc::new(Token));
    egret_workflow::secret_inputs::install_for_tests();
    let (ws, user) = common::seed_workspace(&db).await;
    let ns = ns();
    let listener = format!(
        "id: undecl\nnamespace: {ns}\ninputs:\n  - id: x\n    type: STRING\n    defaults: ok\n\
         tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n\
         triggers:\n  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    \
         states: [SUCCESS]\n    when: \"{{{{ flow.id == 'producer-u' }}}}\"\n    inputs:\n      \
         y: \"{{{{ secret('TOKEN') }}}}\"\n"
    );
    let producer = format!(
        "id: producer-u\nnamespace: {ns}\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    );
    let lid = common::seed_flow(&db, ws, user, &listener, "draft").await;
    let pid = common::seed_flow(&db, ws, user, &producer, "draft").await;
    let src_ex = egret_workflow::worker::start_execution(
        &db.pool,
        ws,
        Some(pid),
        &producer,
        serde_json::json!({}),
    )
    .await
    .unwrap();
    settle_all(&db.pool).await;
    let report = process_events(&db.pool).await.expect("a pass");
    assert_eq!(report.fired, 1, "{report:?}");
    settle_all(&db.pool).await;
    let (ex, inputs): (Uuid, serde_json::Value) =
        sqlx::query_as("SELECT id, inputs FROM workflow.executions WHERE flow_id = $1")
            .bind(lid)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(inputs["y"], egret_workflow::secret_inputs::MASK, "{inputs}");
    assert_eq!(leaks(&db.pool, &[ex, src_ex]).await, 0);
}

/// A render error on a webhook input names the secret's use but never a value.
#[tokio::test]
async fn a_webhook_input_render_error_does_not_echo_the_value() {
    let _serial = SERIAL.lock().await;
    let Some(db) = common::db().await else { return };
    let _ = secrets::install(Arc::new(Token));
    egret_workflow::secret_inputs::install_for_tests();
    let (ws, user) = common::seed_workspace(&db).await;
    let ns = ns();
    let src = format!(
        "id: hook\nnamespace: {ns}\ninputs:\n  - id: x\n    type: STRING\n\
         tasks:\n  - id: real\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n\
         triggers:\n  - id: w\n    type: io.kestra.plugin.core.trigger.Webhook\n    key: fx-key\n    \
         inputs:\n      x: \"{{{{ secret('TOKEN') | number }}}}\"\n"
    );
    common::seed_flow(&db, ws, user, &src, "draft").await;
    let app = common::webhook_app(&db).await;
    let url = format!("/workflows/v1/{ws}/executions/webhook/{ns}/hook/fx-key");
    let res = common::anon(&app, "POST", &url, Some("application/json"), b"{}").await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&body).contains(SENTINEL),
        "{}",
        String::from_utf8_lossy(&body)
    );
}
