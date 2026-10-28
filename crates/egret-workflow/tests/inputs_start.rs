//! Package #3 Task A4: the declared `inputs:` are checked at start (spec D1,
//! D2, D3, D7), on every start path, with the first error in declared order.
//! Requires DATABASE_URL.
//!
//! One test per start path: the API (JSON and multipart), a webhook, a
//! schedule fire, a `Subflow` child; and the two paths that are NOT checked:
//! a replay (Kestra copies the stored inputs) and a Flow-trigger fire (Kestra
//! never validates them; the born-FAILED fire carries `{}`). Plus the two
//! things the check decides beyond yes/no: the coerced object is what is
//! stored, and a SECRET `validator` sees the clear value while the stored one
//! is sealed.
//!
//! **The tests take turns** ([`SERIAL`]) and each starts by emptying the
//! executions and the Flow-trigger outbox and taking every schedule off the
//! queue: a worker tick, a schedule pass and a Flow-trigger pass are all
//! global. The binary owns its database (`egret_test_db`: one clone per
//! process).

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use egret_core::UserId;
use egret_metastore::Db;
use egret_workflow::storage::{self, FileStore};
use egret_workflow::worker::Handlers;
use serde_json::{json, Value};
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const NS: &str = "pkg3.inputs";

fn installed() -> Arc<FileStore> {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        storage::install(Arc::new(FileStore {
            store: Arc::new(egret_storage::MemStore::new()),
            bucket: "egret-test".to_string(),
            prefix: "workflow/".to_string(),
            caps: storage::Caps::default(),
            retention_days: 30,
        }))
        .expect("this binary installs the only store");
    });
    storage::current().expect("installed above")
}

struct Fx {
    db: Db,
    ws: Uuid,
    user: UserId,
    app: common::App,
    _turn: tokio::sync::MutexGuard<'static, ()>,
}

async fn fx() -> Option<Fx> {
    let turn = SERIAL.lock().await;
    let db = common::db().await?;
    installed();
    egret_workflow::secret_inputs::install_for_tests();
    // One statement for the executions, so the NO ACTION foreign key from a
    // fired execution to its source is checked after both are gone.
    for sql in [
        "DELETE FROM workflow.flow_trigger_events",
        "DELETE FROM workflow.executions",
        "DELETE FROM workflow.flow_listeners",
        "UPDATE workflow.triggers SET next_evaluation_date = NULL",
    ] {
        sqlx::query(sql).execute(&db.pool).await.expect(sql);
    }
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage", "workflow:read"]).await;
    let app = common::app(&db).await;
    Some(Fx {
        db,
        ws,
        user,
        app,
        _turn: turn,
    })
}

const BOUNDARY: &str = "pkg3-a4-boundary-7d2e";

enum Part<'a> {
    Text(&'a str, &'a str),
    File(&'a str, &'a str, &'a [u8]),
}

fn multipart_body(parts: &[Part<'_>]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in parts {
        out.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        match p {
            Part::Text(name, value) => {
                out.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                );
                out.extend_from_slice(value.as_bytes());
            }
            Part::File(name, file, bytes) => {
                out.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{file}\"\r\n\
                         Content-Type: text/plain\r\n\r\n"
                    )
                    .as_bytes(),
                );
                out.extend_from_slice(bytes);
            }
        }
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    out
}

impl Fx {
    async fn flow(&self, source: &str) -> Uuid {
        egret_workflow::spec::parse(source)
            .unwrap_or_else(|e| panic!("fixture must parse: {e:?}\n{source}"));
        common::seed_flow(&self.db, self.ws, self.user, source, "draft").await
    }

    fn start_path(&self, flow: Uuid) -> String {
        format!("/api/ws/{}/workflows/flows/{flow}/executions", self.ws)
    }

    async fn start_json(&self, flow: Uuid, inputs: Value) -> axum::response::Response {
        common::post(
            &self.app,
            &self.start_path(flow),
            self.user,
            json!({ "inputs": inputs }),
        )
        .await
    }

    async fn upload(&self, flow: Uuid, parts: &[Part<'_>]) -> axum::response::Response {
        common::send_raw(
            &self.app,
            "POST",
            &self.start_path(flow),
            self.user,
            &format!("multipart/form-data; boundary={BOUNDARY}"),
            multipart_body(parts),
        )
        .await
    }

    async fn executions(&self) -> i64 {
        common::execution_count(&self.db, self.ws).await
    }

    async fn executions_of(&self, flow: Uuid) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM workflow.executions WHERE flow_id = $1")
            .bind(flow)
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }

    async fn inputs_of(&self, execution: Uuid) -> Value {
        sqlx::query_scalar("SELECT inputs FROM workflow.executions WHERE id = $1")
            .bind(execution)
            .fetch_one(&self.db.pool)
            .await
            .expect("inputs")
    }

    /// `(state, message)` of the run of `task` in `execution`.
    async fn run(&self, execution: Uuid, task: &str) -> (String, Option<String>) {
        sqlx::query_as(
            "SELECT state, message FROM workflow.task_runs \
              WHERE execution_id = $1 AND task_id = $2",
        )
        .bind(execution)
        .bind(task)
        .fetch_one(&self.db.pool)
        .await
        .unwrap_or_else(|e| panic!("run {task} of {execution}: {e}"))
    }

    /// Tick until nothing is claimable.
    async fn drain(&self) {
        let reg = Handlers::with_core_tasks(storage::current());
        for _ in 0..64 {
            if egret_workflow::worker::tick(&self.db.pool, 16, &reg)
                .await
                .expect("a worker tick")
                == 0
            {
                return;
            }
        }
        panic!("tick never stopped claiming");
    }
}

async fn id_of(res: axum::response::Response) -> Uuid {
    let got = res.status();
    let dto = common::json(res).await;
    assert_eq!(got, StatusCode::CREATED, "{dto}");
    dto["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no id in {dto}"))
        .parse()
        .expect("uuid")
}

async fn refusal(res: axum::response::Response) -> String {
    let got = res.status();
    let body = common::json(res).await;
    assert_eq!(got, StatusCode::BAD_REQUEST, "{body}");
    body["error"]
        .as_str()
        .expect("an error message")
        .to_string()
}

/// A flow with the given `inputs:` block (2-space list items) and one Log.
fn flow_with(id: &str, inputs: &str) -> String {
    format!(
        "id: {id}\nnamespace: {NS}\ninputs:\n{inputs}tasks:\n  - id: say\n    \
         type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    )
}

const INT_N: &str = "  - id: n\n    type: INT\n    min: 1\n";

// ── The API ─────────────────────────────────────────────────────────────────

/// A JSON start that breaks an input is a 400 naming the input in Kestra's
/// words, and NO execution row is created. The control (a valid value) starts.
#[tokio::test(flavor = "multi_thread")]
async fn an_api_json_start_with_a_bad_input_is_refused_and_creates_nothing() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&flow_with("api-json", INT_N)).await;

    let missing = refusal(fx.start_json(flow, json!({})).await).await;
    assert_eq!(missing, "invalid input: Missing required input:n");
    let not_int = refusal(fx.start_json(flow, json!({"n": "abc"})).await).await;
    assert_eq!(
        not_int,
        "invalid input: Invalid value for input `n`. Cause: Cannot convert value 'abc' to Integer",
    );
    let below = refusal(fx.start_json(flow, json!({"n": 0})).await).await;
    assert_eq!(
        below,
        "invalid input: Invalid value for input `n`. Cause: it must be more than `1`"
    );
    assert_eq!(fx.executions().await, 0, "a refused start stores nothing");

    id_of(fx.start_json(flow, json!({"n": 1})).await).await;
    assert_eq!(fx.executions().await, 1, "control: a valid value starts");
}

/// D2: the COERCED object is stored -- an INT posted as `"5"` is the number
/// 5, a BOOL `"TRUE"` is `true` -- undeclared keys are dropped, and an
/// optional input nobody posted is `null` (Kestra keeps it), while one with a
/// `defaults` is left for the render to fill.
#[tokio::test(flavor = "multi_thread")]
async fn the_coerced_object_is_what_is_stored() {
    let Some(fx) = fx().await else { return };
    let flow = fx
        .flow(&flow_with(
            "coerced",
            "  - id: n\n    type: INT\n  - id: b\n    type: BOOL\n\
             \x20 - id: opt\n    type: STRING\n    required: false\n\
             \x20 - id: d\n    type: INT\n    defaults: 7\n",
        ))
        .await;
    let ex = id_of(
        fx.start_json(flow, json!({"n": "5", "b": "TRUE", "stray": 1}))
            .await,
    )
    .await;
    assert_eq!(
        fx.inputs_of(ex).await,
        json!({"n": 5, "b": true, "opt": null}),
    );
}

/// The multipart start: every value is a string part, a FILE part is
/// uploaded first and its `kestra://` URI is put into `inputs[id]` BEFORE the
/// check, so a required FILE input is satisfied by its upload (A2 concern 5).
/// A missing required FILE and a bad text part are refused with nothing
/// created.
#[tokio::test(flavor = "multi_thread")]
async fn a_multipart_start_is_checked_and_a_required_file_is_satisfied_by_its_upload() {
    let Some(fx) = fx().await else { return };
    let flow = fx
        .flow(&flow_with(
            "multipart",
            "  - id: f\n    type: FILE\n  - id: n\n    type: INT\n",
        ))
        .await;

    let ex = id_of(
        fx.upload(
            flow,
            &[Part::File("f", "a.txt", b"bytes"), Part::Text("n", "7")],
        )
        .await,
    )
    .await;
    let stored = fx.inputs_of(ex).await;
    assert_eq!(stored["n"], json!(7), "the text part, coerced: {stored}");
    let uri = stored["f"].as_str().expect("the FILE input is its URI");
    assert!(
        uri.starts_with("kestra://") && uri.ends_with("/inputs/f/a.txt"),
        "{uri}"
    );

    let missing_file = refusal(fx.upload(flow, &[Part::Text("n", "7")]).await).await;
    assert_eq!(missing_file, "invalid input: Missing required input:f");
    let bad_text = refusal(
        fx.upload(
            flow,
            &[Part::File("f", "b.txt", b"bytes"), Part::Text("n", "x")],
        )
        .await,
    )
    .await;
    assert_eq!(
        bad_text,
        "invalid input: Invalid value for input `n`. Cause: Cannot convert value 'x' to Integer"
    );
    assert_eq!(fx.executions().await, 1, "only the control started");
}

/// A SECRET `validator` runs on the CLEAR value (before `split` masks it): a
/// value that matches is accepted -- the mask `******` would not match -- and
/// is then stored masked and sealed; one that does not is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_secret_validator_sees_the_clear_value_and_the_stored_value_is_sealed() {
    let Some(fx) = fx().await else { return };
    let flow = fx
        .flow(&flow_with(
            "secret",
            "  - id: pw\n    type: SECRET\n    validator: \"[a-z]+\"\n",
        ))
        .await;

    let ex = id_of(fx.start_json(flow, json!({"pw": "hunter"})).await).await;
    let (inputs, sealed): (Value, Option<Vec<u8>>) =
        sqlx::query_as("SELECT inputs, secret_inputs FROM workflow.executions WHERE id = $1")
            .bind(ex)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(inputs, json!({"pw": "******"}), "masked in `inputs`");
    let sealed = sealed.expect("the clear value is sealed");
    assert!(
        !String::from_utf8_lossy(&sealed).contains("hunter"),
        "sealed, not stored in clear"
    );

    let refused = refusal(fx.start_json(flow, json!({"pw": "Hunter2"})).await).await;
    assert_eq!(
        refused,
        "invalid input: Invalid value for input `pw`. Cause: pw: it must match the pattern `[a-z]+`"
    );
    assert_eq!(fx.executions().await, 1);
}

// ── A webhook ───────────────────────────────────────────────────────────────

/// A webhook's rendered `inputs` are checked: a bad one answers 400 with the
/// message (Kestra: 422, empty body -- recorded divergence D3) and starts
/// nothing. Control: a valid rendered value starts, coerced.
#[tokio::test(flavor = "multi_thread")]
async fn a_webhook_whose_rendered_inputs_are_refused_answers_400() {
    let Some(fx) = fx().await else { return };
    let hook = |id: &str, key: &str, value: &str| {
        format!(
            "id: {id}\nnamespace: {NS}\ninputs:\n  - id: n\n    type: INT\n\
             tasks:\n  - id: say\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n\
             triggers:\n  - id: hook\n    type: io.kestra.plugin.core.trigger.Webhook\n    \
             key: {key}\n    inputs:\n      n: \"{value}\"\n"
        )
    };
    let bad = fx.flow(&hook("hook-bad", "bad-key", "{{ flow.id }}")).await;
    let good = fx.flow(&hook("hook-good", "good-key", "4{{ 2 }}")).await;
    let app = common::webhook_app(&fx.db).await;
    let url = |flow: &str, key: &str| {
        format!(
            "/workflows/v1/{}/executions/webhook/{NS}/{flow}/{key}",
            fx.ws
        )
    };

    let res = common::anon(
        &app,
        "POST",
        &url("hook-bad", "bad-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = common::json(res).await;
    assert_eq!(
        body["error"],
        "invalid input: Invalid value for input `n`. Cause: Cannot convert value 'hook-bad' to \
         Integer",
    );
    assert_eq!(fx.executions_of(bad).await, 0);

    let res = common::anon(
        &app,
        "POST",
        &url("hook-good", "good-key"),
        Some("application/json"),
        b"{}",
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK, "control");
    let stored: Value =
        sqlx::query_scalar("SELECT inputs FROM workflow.executions WHERE flow_id = $1")
            .bind(good)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(stored, json!({"n": 42}));
}

// ── A schedule fire ─────────────────────────────────────────────────────────

/// A schedule whose literal `inputs` break the declaration is refused at fire
/// and keeps today's `Refused` handling: no execution, the trigger taken off
/// the queue, the reason in `last_error`.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_fire_with_a_refused_input_is_taken_off_the_queue() {
    let Some(fx) = fx().await else { return };
    let flow = fx
        .flow(&format!(
            "id: sched\nnamespace: {NS}\ninputs:\n  - id: n\n    type: INT\n\
             triggers:\n  - id: s\n    type: io.kestra.plugin.core.trigger.Schedule\n    \
             cron: \"*/1 * * * *\"\n    inputs:\n      n: abc\n\
             tasks:\n  - id: say\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
        ))
        .await;
    common::make_due(&fx.db, flow, "s", 1).await;

    let report = egret_workflow::scheduler::fire_due(&fx.db.pool)
        .await
        .expect("a pass");
    assert_eq!(
        report,
        egret_workflow::scheduler::PassReport {
            errored: 1,
            ..Default::default()
        }
    );
    assert_eq!(fx.executions_of(flow).await, 0);
    let (next, err): (Option<chrono::DateTime<chrono::Utc>>, Option<String>) = sqlx::query_as(
        "SELECT next_evaluation_date, last_error FROM workflow.triggers \
          WHERE flow_id = $1 AND trigger_id = 's'",
    )
    .bind(flow)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(next, None, "off the queue");
    let err = err.expect("the refusal is recorded");
    assert!(
        err.contains("Invalid value for input `n`. Cause: Cannot convert value 'abc' to Integer"),
        "{err}"
    );
}

// ── A Subflow child ─────────────────────────────────────────────────────────

/// The child's declaration is checked before its row is inserted: a refusal
/// fails the PARENT's `Subflow` run with `subflow inputs: <message>` and
/// leaves no child. Control: a valid value starts the child, coerced.
#[tokio::test(flavor = "multi_thread")]
async fn a_subflow_child_refused_at_start_fails_the_parents_run() {
    let Some(fx) = fx().await else { return };
    let child = fx.flow(&flow_with("child", INT_N)).await;
    let parent = |id: &str, value: &str| {
        format!(
            "id: {id}\nnamespace: {NS}\ntasks:\n  - id: call\n    \
             type: io.kestra.plugin.core.flow.Subflow\n    namespace: {NS}\n    \
             flowId: child\n    inputs:\n      n: \"{value}\"\n"
        )
    };
    let bad = fx.flow(&parent("parent-bad", "zero")).await;
    let good = fx.flow(&parent("parent-good", "3")).await;

    let p = id_of(fx.start_json(bad, json!({})).await).await;
    fx.drain().await;
    let (state, message) = fx.run(p, "call").await;
    assert_eq!(state, "FAILED");
    assert_eq!(
        message.as_deref(),
        Some(
            "subflow inputs: Invalid value for input `n`. Cause: Cannot convert value 'zero' to \
             Integer"
        ),
    );
    assert_eq!(fx.executions_of(child).await, 0, "no child row");

    let p = id_of(fx.start_json(good, json!({})).await).await;
    fx.drain().await;
    let kids: Vec<Value> =
        sqlx::query_scalar("SELECT inputs FROM workflow.executions WHERE flow_id = $1")
            .bind(child)
            .fetch_all(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(kids, vec![json!({"n": 3})], "control: started, coerced");
    assert_eq!(fx.run(p, "call").await.0, "SUCCESS");
}

// ── Not checked: a replay ───────────────────────────────────────────────────

/// A replay starts with the source's STORED inputs and is not re-checked
/// (Kestra's replay copies them). The probe: a stored value the check would
/// refuse (written by SQL, as an execution stored before package #3 could
/// hold) is replayed as it is.
#[tokio::test(flavor = "multi_thread")]
async fn a_replay_is_not_re_checked() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&flow_with("replayed", INT_N)).await;
    let source = id_of(fx.start_json(flow, json!({"n": 2})).await).await;
    fx.drain().await;
    sqlx::query(
        "UPDATE workflow.executions SET inputs = '{\"n\": \"abc\"}'::jsonb, state = 'SUCCESS' \
          WHERE id = $1",
    )
    .bind(source)
    .execute(&fx.db.pool)
    .await
    .unwrap();
    assert!(
        refusal(fx.start_json(flow, json!({"n": "abc"})).await)
            .await
            .contains("Invalid value for input `n`"),
        "control: the same value posted to the start is refused"
    );

    let res = common::post_no_body(
        &fx.app,
        &format!("/api/ws/{}/workflows/executions/{source}/replay", fx.ws),
        fx.user,
    )
    .await;
    let replay = id_of(res).await;
    assert_eq!(fx.inputs_of(replay).await, json!({"n": "abc"}));
}

// ── Not checked: a Flow trigger ─────────────────────────────────────────────

/// A Flow-trigger fire is not checked (Kestra renders a Flow trigger's inputs
/// and never validates them): a listener whose rendered value its declaration
/// refuses still fires, storing the value as rendered. And the born-FAILED
/// fire -- `inputs` that do not render, so `{}`, for a listener whose input
/// is required -- is still created `CREATED -> FAILED`, not refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_flow_trigger_fire_is_not_checked_and_the_born_failed_fire_still_works() {
    let Some(fx) = fx().await else { return };
    let producer_src = format!(
        "id: producer\nnamespace: {NS}\ntasks:\n  - id: a\n    \
         type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    );
    let producer = fx.flow(&producer_src).await;
    let listener = |id: &str, value: &str| {
        format!(
            "id: {id}\nnamespace: {NS}\ninputs:\n  - id: n\n    type: INT\n\
             triggers:\n  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    \
             states: [SUCCESS]\n    when: \"{{{{ flow.id == 'producer' }}}}\"\n    \
             inputs:\n      n: \"{value}\"\n\
             tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
        )
    };
    let unchecked = fx.flow(&listener("unchecked", "abc")).await;
    let born_failed = fx
        .flow(&listener("born-failed", "{{ trigger.executionId }}"))
        .await;

    let exec = egret_workflow::worker::start_execution(
        &fx.db.pool,
        fx.ws,
        Some(producer),
        &producer_src,
        json!({}),
    )
    .await
    .expect("start the producer");
    common::drive_to(&fx.db, exec, "SUCCESS").await;
    let report = egret_workflow::flow_trigger::process_events(&fx.db.pool)
        .await
        .expect("a pass");
    assert_eq!(
        (report.fired, report.born_failed),
        (1, 1),
        "`unchecked` fired, `born-failed` born FAILED: {report:?}"
    );

    let (state, inputs): (String, Value) =
        sqlx::query_as("SELECT state, inputs FROM workflow.executions WHERE flow_id = $1")
            .bind(unchecked)
            .fetch_one(&fx.db.pool)
            .await
            .expect("the unchecked listener fired");
    assert_eq!(inputs, json!({"n": "abc"}), "stored as rendered, unchecked");
    assert_ne!(
        state, "FAILED",
        "fired; the bad value surfaces only at render"
    );

    let (state, history, inputs): (String, Value, Value) =
        sqlx::query_as("SELECT state, history, inputs FROM workflow.executions WHERE flow_id = $1")
            .bind(born_failed)
            .fetch_one(&fx.db.pool)
            .await
            .expect("the born-FAILED fire was created, not refused");
    assert_eq!(state, "FAILED");
    assert_eq!(history, json!(["CREATED", "FAILED"]));
    assert_eq!(inputs, json!({}));
}
