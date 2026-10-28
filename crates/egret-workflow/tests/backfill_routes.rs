//! W7f Task 6: reading backfills (spec §3.9, D12, D15) -- the `backfill`,
//! `can_backfill` and `backfill_refusal` fields of a trigger entry, the
//! workspace list's `backfill` filter, `missed_ticks` during a backfill --
//! and the fire after a re-enable during a backfill (§3.8, D11) -- and, from
//! Task 7, the write routes (§3.4, §3.5, §3.10): create with every refusal
//! of §5, pause and resume, delete and its resume, the bulk routes, the audit
//! events. Requires DATABASE_URL.
//!
//! **The tests in this binary run one at a time** (`SERIAL`): the re-enable
//! test runs a `fire_due` pass, which is global, so it first takes every
//! trigger an earlier test left behind off the queue (`tests/backfill_fire.rs`'s
//! reason), and no other test of this binary may be running then.
//!
//! The read tests stage a backfill by SQL (`common::stage_backfill`), the
//! write tests through the create route. Window ticks are built from the database clock and
//! lie in the past, so they do not move however long a test takes.

mod common;

use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use egret_core::UserId;
use egret_metastore::Db;
use egret_workflow::routes::TriggerFlags;
use serde_json::{json, Value};
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const SCHEDULE: &str = "io.kestra.plugin.core.trigger.Schedule";
const WEBHOOK: &str = "io.kestra.plugin.core.trigger.Webhook";

const FLAGS: TriggerFlags = TriggerFlags {
    scheduler_enabled: true,
    worker_enabled: false,
    webhooks_enabled: true,
};

fn src(id: &str, triggers: &str) -> String {
    let s = format!(
        "id: {id}\nnamespace: w7f.read\ntriggers:\n{triggers}tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    );
    egret_workflow::spec::parse(&s).unwrap_or_else(|e| panic!("fixture must parse: {e:?}\n{s}"));
    s
}

fn schedule(id: &str) -> String {
    format!("  - id: {id}\n    type: {SCHEDULE}\n    cron: \"*/1 * * * *\"\n")
}

struct Fx {
    db: Db,
    ws: Uuid,
    user: UserId,
    app: common::App,
    _serial: tokio::sync::MutexGuard<'static, ()>,
}

async fn fx() -> Option<Fx> {
    let serial = SERIAL.lock().await;
    let db = common::db().await?;
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read", "workflow:manage"]).await;
    let app = common::app_with_trigger_flags(&db, FLAGS).await;
    Some(Fx {
        db,
        ws,
        user,
        app,
        _serial: serial,
    })
}

impl Fx {
    async fn flow(&self, id: &str, triggers: &str) -> Uuid {
        common::seed_flow(&self.db, self.ws, self.user, &src(id, triggers), "draft").await
    }

    async fn sql(&self, q: &'static str, flow: Uuid) {
        sqlx::query(q)
            .bind(flow)
            .execute(&self.db.pool)
            .await
            .expect(q);
    }

    /// The database's `now()` truncated to the minute: a tick of `*/1`.
    async fn minute(&self) -> DateTime<Utc> {
        sqlx::query_scalar("SELECT date_trunc('minute', now())")
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }

    async fn now(&self) -> DateTime<Utc> {
        sqlx::query_scalar("SELECT now()")
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }

    /// A user named `name` holding `perms` in this workspace. The PDP reads
    /// the grants when it is built: a caller that sends this user's requests
    /// rebuilds the app after ([`Fx::rebuild_app`]).
    async fn user(&self, name: &str, perms: &[&str]) -> UserId {
        let u = UserId(
            sqlx::query_scalar(
                "INSERT INTO users (email, name, provider, status, password_hash) \
                 VALUES ($1, $2, 'local', 'active', 'x') RETURNING id",
            )
            .bind(format!("b_{}@t.local", Uuid::new_v4().simple()))
            .bind(name)
            .fetch_one(&self.db.pool)
            .await
            .unwrap(),
        );
        common::grant(&self.db, self.ws, u, perms).await;
        u
    }

    async fn rebuild_app(&mut self) {
        self.app = common::app_with_trigger_flags(&self.db, FLAGS).await;
    }

    async fn next_date(&self, flow: Uuid, trig: &str) -> Option<DateTime<Utc>> {
        sqlx::query_scalar(
            "SELECT next_evaluation_date FROM workflow.triggers \
              WHERE flow_id = $1 AND trigger_id = $2",
        )
        .bind(flow)
        .bind(trig)
        .fetch_one(&self.db.pool)
        .await
        .unwrap()
    }

    /// A backfill over `(start, end)` from its first tick, with no inputs
    /// and no labels.
    async fn backfill(
        &self,
        flow: Uuid,
        trig: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Uuid {
        let ticks = ((end - start).num_minutes() + 1) as i32;
        common::stage_backfill(
            &self.db,
            flow,
            trig,
            (start, end),
            start,
            ticks,
            json!({}),
            json!([]),
        )
        .await
    }

    async fn flow_triggers_as(&self, user: UserId, flow: Uuid) -> Value {
        let res = common::get(
            &self.app,
            &format!("/api/ws/{}/workflows/flows/{flow}/triggers", self.ws),
            user,
        )
        .await;
        let status = res.status();
        let body = common::json(res).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    async fn flow_triggers(&self, flow: Uuid) -> Value {
        self.flow_triggers_as(self.user, flow).await
    }

    async fn list_res(&self, query: &str) -> (StatusCode, Value) {
        let res = common::get(
            &self.app,
            &format!("/api/ws/{}/workflows/triggers{query}", self.ws),
            self.user,
        )
        .await;
        let status = res.status();
        (status, common::json(res).await)
    }

    async fn list(&self, query: &str) -> Value {
        let (status, body) = self.list_res(query).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }
}

fn entry<'a>(body: &'a Value, id: &str) -> &'a Value {
    body["triggers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["trigger_id"] == json!(id))
        .unwrap_or_else(|| panic!("no trigger {id} in {body}"))
}

fn ts(v: &Value) -> DateTime<Utc> {
    v.as_str()
        .unwrap_or_else(|| panic!("not a timestamp: {v}"))
        .parse()
        .unwrap()
}

/// `"flow_id/trigger_id"` of every row, sorted.
fn names(page: &Value) -> Vec<String> {
    let mut v: Vec<String> = page["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            format!(
                "{}/{}",
                r["flow_id"].as_str().unwrap(),
                r["trigger_id"].as_str().unwrap()
            )
        })
        .collect();
    v.sort();
    v
}

fn row<'a>(page: &'a Value, flow_id: &str) -> &'a Value {
    page["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["flow_id"] == json!(flow_id))
        .unwrap_or_else(|| panic!("no row of {flow_id} in {page}"))
}

/// Test 2: every field of `BackfillDto`, for a running and a paused backfill,
/// the creator's and the pauser's names resolved.
#[tokio::test]
async fn an_entry_carries_its_backfill() {
    let Some(fx) = fx().await else { return };
    let flow = fx
        .flow(
            "fields",
            &format!("{}{}", schedule("run"), schedule("held")),
        )
        .await;
    let cara = fx.user("Cara", &["workflow:read"]).await;
    let pat = fx.user("Pat", &["workflow:read"]).await;
    let m = fx.minute().await;
    let w0 = m - Duration::minutes(10);

    // Running: from its first tick, created by Cara, with inputs and labels.
    let prev_run = fx.next_date(flow, "run").await.unwrap();
    let run = common::stage_backfill(
        &fx.db,
        flow,
        "run",
        (w0, w0 + Duration::minutes(5)),
        w0,
        6,
        json!({"x": 1, "who": "bf"}),
        json!([{"key": "team", "value": "a"}, {"key": "env", "value": "dev"}]),
    )
    .await;
    sqlx::query("UPDATE workflow.trigger_backfills SET created_by = $2 WHERE flow_id = $1 AND trigger_id = 'run'")
        .bind(flow)
        .bind(cara.0)
        .execute(&fx.db.pool)
        .await
        .unwrap();

    // Paused by Pat, halfway: window w0..w0+4 (5 ticks), at w0+2, 2 done.
    let prev_held = fx.next_date(flow, "held").await.unwrap();
    let held = common::stage_backfill(
        &fx.db,
        flow,
        "held",
        (w0, w0 + Duration::minutes(4)),
        w0 + Duration::minutes(2),
        5,
        json!({}),
        json!([]),
    )
    .await;
    let paused_at = fx.now().await - Duration::minutes(1);
    sqlx::query(
        "UPDATE workflow.trigger_backfills SET paused = true, paused_by = $2, paused_at = $3, \
                ticks_done = 2 \
          WHERE flow_id = $1 AND trigger_id = 'held'",
    )
    .bind(flow)
    .bind(pat.0)
    .bind(paused_at)
    .execute(&fx.db.pool)
    .await
    .unwrap();

    let body = fx.flow_triggers(flow).await;
    let created = |trig: &str| {
        let db = fx.db.clone();
        let trig = trig.to_string();
        async move {
            let r = common::backfill_row(&db, flow, &trig).await.unwrap();
            ts(&r["created_at"])
        }
    };

    let b = &entry(&body, "run")["backfill"];
    assert_eq!(b["id"], json!(run), "{b}");
    assert_eq!(ts(&b["start"]), w0);
    assert_eq!(ts(&b["end"]), w0 + Duration::minutes(5));
    assert_eq!(ts(&b["current_tick"]), w0);
    assert_eq!(b["paused"], json!(false));
    assert_eq!(b["paused_by_name"], json!(null));
    assert_eq!(b["paused_at"], json!(null));
    assert_eq!(b["inputs"], json!({"x": 1, "who": "bf"}));
    assert_eq!(
        b["labels"],
        json!([{"key": "team", "value": "a"}, {"key": "env", "value": "dev"}])
    );
    assert_eq!(b["created_by_name"], json!("Cara"));
    assert_eq!(ts(&b["created_at"]), created("run").await);
    assert_eq!(ts(&b["previous_next_evaluation_date"]), prev_run);
    assert_eq!(b["total_ticks"], json!(6));
    assert_eq!(b["ticks_done"], json!(0));
    assert_eq!(b["progress"], json!(0));
    assert_eq!(b.as_object().unwrap().len(), 15, "no field unasserted: {b}");

    let b = &entry(&body, "held")["backfill"];
    assert_eq!(b["id"], json!(held), "{b}");
    assert_eq!(ts(&b["start"]), w0);
    assert_eq!(ts(&b["end"]), w0 + Duration::minutes(4));
    assert_eq!(ts(&b["current_tick"]), w0 + Duration::minutes(2));
    assert_eq!(b["paused"], json!(true));
    assert_eq!(b["paused_by_name"], json!("Pat"));
    assert_eq!(ts(&b["paused_at"]), paused_at);
    assert_eq!(b["inputs"], json!({}));
    assert_eq!(b["labels"], json!([]));
    assert_eq!(b["created_by_name"], json!(null), "no creator recorded");
    assert_eq!(ts(&b["created_at"]), created("held").await);
    assert_eq!(ts(&b["previous_next_evaluation_date"]), prev_held);
    assert_eq!(b["total_ticks"], json!(5));
    assert_eq!(b["ticks_done"], json!(2));
    assert_eq!(b["progress"], json!(50));

    // The schedule's own date is the backfill's tick.
    assert_eq!(
        ts(&entry(&body, "held")["schedule"]["next_evaluation_date"]),
        w0 + Duration::minutes(2)
    );
}

/// Test 3: each cause of `backfill_refusal` alone, then two causes at once
/// (the first wins).
#[tokio::test]
async fn can_backfill_says_why_not_first_cause_first() {
    let Some(mut fx) = fx().await else { return };
    let reader = fx.user("Rae", &["workflow:read"]).await;
    fx.rebuild_app().await;
    let w0 = fx.minute().await - Duration::minutes(10);
    let refusal = |body: &Value, trig: &str| {
        let e = entry(body, trig);
        (e["can_backfill"].clone(), e["backfill_refusal"].clone())
    };
    let no = |code: &str| (json!(false), json!(code));

    // None: usable.
    let ok = fx.flow("ok", &schedule("s")).await;
    let body = fx.flow_triggers(ok).await;
    assert_eq!(refusal(&body, "s"), (json!(true), json!(null)));
    assert_eq!(entry(&body, "s")["backfill"], json!(null));

    // A trashed flow.
    let trashed = fx.flow("trashed", &schedule("s")).await;
    fx.sql(
        "UPDATE workflow.flows SET deleted_at = now() WHERE id = $1",
        trashed,
    )
    .await;
    assert_eq!(
        refusal(&fx.flow_triggers(trashed).await, "s"),
        no("flow_deleted")
    );

    // A webhook: not a schedule, and no backfill object.
    let hook = fx
        .flow(
            "hook",
            &format!("  - id: h\n    type: {WEBHOOK}\n    key: k\n"),
        )
        .await;
    let body = fx.flow_triggers(hook).await;
    assert_eq!(refusal(&body, "h"), no("not_schedule"));
    assert_eq!(entry(&body, "h")["backfill"], json!(null));

    // `workflow:read` only.
    assert_eq!(
        refusal(&fx.flow_triggers_as(reader, ok).await, "s"),
        no("forbidden")
    );

    // Disabled, three ways: the YAML, the switch, the flow's state column.
    let yaml = fx
        .flow("yaml", &format!("{}    disabled: true\n", schedule("s")))
        .await;
    assert_eq!(refusal(&fx.flow_triggers(yaml).await, "s"), no("disabled"));
    let switched = fx.flow("switched", &schedule("s")).await;
    common::switch_off(&fx.db, switched, "s").await;
    assert_eq!(
        refusal(&fx.flow_triggers(switched).await, "s"),
        no("disabled")
    );
    let state = fx.flow("state", &schedule("s")).await;
    fx.sql(
        "UPDATE workflow.flows SET state = 'disabled' WHERE id = $1",
        state,
    )
    .await;
    assert_eq!(refusal(&fx.flow_triggers(state).await, "s"), no("disabled"));

    // Off the queue.
    let off = fx.flow("off", &schedule("s")).await;
    fx.sql(
        "UPDATE workflow.triggers SET next_evaluation_date = NULL WHERE flow_id = $1",
        off,
    )
    .await;
    assert_eq!(refusal(&fx.flow_triggers(off).await, "s"), no("off_queue"));

    // A backfill already runs.
    let running = fx.flow("running", &schedule("s")).await;
    fx.backfill(running, "s", w0, w0 + Duration::minutes(3))
        .await;
    assert_eq!(
        refusal(&fx.flow_triggers(running).await, "s"),
        no("running")
    );

    // Two causes: switched off during a backfill is `disabled`, not `running`;
    // a reader on it is `forbidden`.
    let both = fx.flow("both", &schedule("s")).await;
    fx.backfill(both, "s", w0, w0 + Duration::minutes(3)).await;
    common::switch_off(&fx.db, both, "s").await;
    assert_eq!(refusal(&fx.flow_triggers(both).await, "s"), no("disabled"));
    assert_eq!(
        refusal(&fx.flow_triggers_as(reader, both).await, "s"),
        no("forbidden")
    );
}

/// Test 4: during a backfill `missed_ticks` counts the regular ticks waiting
/// (from `previous_next_evaluation_date`), not the window's.
#[tokio::test]
async fn missed_ticks_during_a_backfill_count_the_regular_schedule() {
    let Some(fx) = fx().await else { return };
    let flow = fx
        .flow(
            "missed",
            &format!("{}{}", schedule("due"), schedule("ahead")),
        )
        .await;
    // Three regular ticks due (`make_due` leaves at least ten seconds before
    // the next minute boundary).
    common::make_due(&fx.db, flow, "due", 3).await;
    let m = fx.minute().await;
    fx.sql(
        "UPDATE workflow.triggers SET next_evaluation_date = date_trunc('minute', now()) + interval '5 minutes' \
          WHERE flow_id = $1 AND trigger_id = 'ahead'",
        flow,
    )
    .await;
    // Both walk a window hours back.
    let (ws, we) = (m - Duration::hours(3), m - Duration::hours(2));
    fx.backfill(flow, "due", ws, we).await;
    fx.backfill(flow, "ahead", ws, we).await;

    let body = fx.flow_triggers(flow).await;
    let s = &entry(&body, "due")["schedule"];
    assert_eq!(ts(&s["next_evaluation_date"]), ws, "the window's tick");
    assert_eq!(s["missed_ticks"], json!({"count": 3, "more": false}), "{s}");
    let s = &entry(&body, "ahead")["schedule"];
    assert_eq!(ts(&s["next_evaluation_date"]), ws);
    assert_eq!(s["missed_ticks"], json!(null), "{s}");

    // The list counts the same way per row.
    let page = fx.list("?q=missed").await;
    let rows = page["rows"].as_array().unwrap();
    let by = |trig: &str| {
        rows.iter()
            .find(|r| r["trigger_id"] == json!(trig))
            .unwrap_or_else(|| panic!("no {trig} in {page}"))["schedule"]["missed_ticks"]
            .clone()
    };
    assert_eq!(by("due"), json!({"count": 3, "more": false}));
    assert_eq!(by("ahead"), json!(null));
}

/// Test 5: list rows carry `backfill`; the `backfill` filter.
#[tokio::test]
async fn the_list_carries_and_filters_backfills() {
    let Some(fx) = fx().await else { return };
    let w0 = fx.minute().await - Duration::minutes(10);
    let running = fx.flow("bf_running", &schedule("s")).await;
    let paused = fx.flow("bf_paused", &schedule("s")).await;
    fx.flow("bf_none", &schedule("s")).await;
    let run_id = fx
        .backfill(running, "s", w0, w0 + Duration::minutes(3))
        .await;
    let held_id = fx
        .backfill(paused, "s", w0, w0 + Duration::minutes(3))
        .await;
    fx.sql(
        "UPDATE workflow.trigger_backfills SET paused = true, paused_at = now() WHERE flow_id = $1",
        paused,
    )
    .await;

    let page = fx.list("").await;
    assert_eq!(
        names(&page),
        vec!["bf_none/s", "bf_paused/s", "bf_running/s"]
    );
    assert_eq!(row(&page, "bf_running")["backfill"]["id"], json!(run_id));
    assert_eq!(row(&page, "bf_running")["backfill"]["paused"], json!(false));
    assert_eq!(row(&page, "bf_paused")["backfill"]["id"], json!(held_id));
    assert_eq!(row(&page, "bf_paused")["backfill"]["paused"], json!(true));
    assert_eq!(row(&page, "bf_none")["backfill"], json!(null));
    assert_eq!(row(&page, "bf_none")["can_backfill"], json!(true));
    assert_eq!(
        row(&page, "bf_running")["backfill_refusal"],
        json!("running")
    );

    assert_eq!(
        names(&fx.list("?backfill=running").await),
        vec!["bf_running/s"]
    );
    assert_eq!(
        names(&fx.list("?backfill=paused").await),
        vec!["bf_paused/s"]
    );
    assert_eq!(
        names(&fx.list("?backfill=any").await),
        vec!["bf_paused/s", "bf_running/s"]
    );
    assert_eq!(fx.list("?backfill=any").await["total"], json!(2));
    assert_eq!(
        names(&fx.list("?backfill=").await).len(),
        3,
        "empty is no filter"
    );
    let (status, body) = fx.list_res("?backfill=bogus").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

/// Test 6's fire: switched off and on during a backfill (skip), one pass
/// fires the backfill's `current_tick` -- it resumes where it stopped, and no
/// window tick is lost. (The dates and the answer are asserted in
/// `tests/triggers_routes.rs`, beside W7e's re-enable tests; the pass is
/// global, so it lives in this serial binary.)
#[tokio::test]
async fn a_re_enabled_backfill_fires_where_it_stopped() {
    let Some(fx) = fx().await else { return };
    sqlx::query("UPDATE workflow.triggers SET next_evaluation_date = NULL")
        .execute(&fx.db.pool)
        .await
        .unwrap();
    let flow = fx.flow("refire", &schedule("s")).await;
    common::make_due(&fx.db, flow, "s", 3).await;
    let w0 = fx.minute().await - Duration::minutes(30);
    let id = fx.backfill(flow, "s", w0, w0 + Duration::minutes(5)).await;
    let path = format!(
        "/api/ws/{}/workflows/flows/{flow}/triggers/s/disabled",
        fx.ws
    );
    for disabled in [true, false] {
        let res = common::put(
            &fx.app,
            &path,
            fx.user,
            json!({"disabled": disabled, "recover_missed": false}),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
    }
    assert_eq!(
        fx.next_date(flow, "s").await,
        Some(w0),
        "the window's tick kept"
    );

    let report = egret_workflow::scheduler::fire_due(&fx.db.pool)
        .await
        .expect("a pass");
    assert_eq!(report.fired, 1, "{report:?}");
    assert_eq!(common::scheduled_dates(&fx.db, flow).await, vec![w0]);
    let fired: Option<Uuid> =
        sqlx::query_scalar("SELECT backfill_id FROM workflow.executions WHERE flow_id = $1")
            .bind(flow)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(fired, Some(id));
    let b = common::backfill_row(&fx.db, flow, "s").await.unwrap();
    assert_eq!(ts(&b["current_tick"]), w0 + Duration::minutes(1));
    assert_eq!(b["ticks_done"], json!(1));
}

// ── Task 7: the write routes (§3.4, §3.5, §3.10) ─────────────────────────────

/// A flow declaring `inputs` (YAML list items) and `triggers`.
fn src_inputs(id: &str, inputs: &str, triggers: &str) -> String {
    let s = format!(
        "id: {id}\nnamespace: w7f.write\ninputs:\n{inputs}triggers:\n{triggers}tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    );
    egret_workflow::spec::parse(&s).unwrap_or_else(|e| panic!("fixture must parse: {e:?}\n{s}"));
    s
}

/// `region` (STRING, defaulted), `req` (STRING, required, no default) and
/// `upload` (FILE, optional).
const INPUTS: &str = "  - id: region\n    type: STRING\n    defaults: us\n  - id: req\n    type: STRING\n  - id: upload\n    type: FILE\n    required: false\n";

fn cron(id: &str, cron: &str, extra: &str) -> String {
    format!("  - id: {id}\n    type: {SCHEDULE}\n    cron: \"{cron}\"\n{extra}")
}

fn rfc(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

impl Fx {
    async fn flow_src(&self, source: &str) -> Uuid {
        common::seed_flow(&self.db, self.ws, self.user, source, "draft").await
    }

    fn bf_path(&self, flow: Uuid, trig: &str) -> String {
        format!(
            "/api/ws/{}/workflows/flows/{flow}/triggers/{trig}/backfill",
            self.ws
        )
    }

    async fn create_as(
        &self,
        user: UserId,
        flow: Uuid,
        trig: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let res = common::post(&self.app, &self.bf_path(flow, trig), user, body).await;
        let status = res.status();
        (status, common::json(res).await)
    }

    async fn create(&self, flow: Uuid, trig: &str, body: Value) -> (StatusCode, Value) {
        self.create_as(self.user, flow, trig, body).await
    }

    async fn pause(&self, flow: Uuid, trig: &str, paused: bool) -> (StatusCode, Value) {
        let res = common::put(
            &self.app,
            &format!("{}/paused", self.bf_path(flow, trig)),
            self.user,
            json!({ "paused": paused }),
        )
        .await;
        let status = res.status();
        (status, common::json(res).await)
    }

    async fn delete_bf(&self, flow: Uuid, trig: &str) -> (StatusCode, Value) {
        let res = common::delete(&self.app, &self.bf_path(flow, trig), self.user).await;
        let status = res.status();
        (status, common::json(res).await)
    }

    async fn bulk(&self, method: &str, suffix: &str, body: Value) -> (StatusCode, Value) {
        let path = format!("/api/ws/{}/workflows/triggers/backfill/{suffix}", self.ws);
        let res = match method {
            "PUT" => common::put(&self.app, &path, self.user, body).await,
            _ => common::post(&self.app, &path, self.user, body).await,
        };
        let status = res.status();
        (status, common::json(res).await)
    }

    /// `(the trigger row, the backfill row)`, whole: "nothing was written"
    /// compares two of these.
    async fn rows(&self, flow: Uuid, trig: &str) -> (Option<Value>, Option<Value>) {
        let t: Option<Value> = sqlx::query_scalar(
            "SELECT row_to_json(t) FROM workflow.triggers t \
              WHERE t.flow_id = $1 AND t.trigger_id = $2",
        )
        .bind(flow)
        .bind(trig)
        .fetch_optional(&self.db.pool)
        .await
        .unwrap();
        (t, common::backfill_row(&self.db, flow, trig).await)
    }

    /// Every trigger of every earlier test off the queue: `fire_due` is global.
    async fn quiet_the_queue(&self) {
        sqlx::query("UPDATE workflow.triggers SET next_evaluation_date = NULL")
            .execute(&self.db.pool)
            .await
            .unwrap();
    }

    async fn pass(&self) -> egret_workflow::scheduler::PassReport {
        egret_workflow::scheduler::fire_due(&self.db.pool)
            .await
            .expect("a pass")
    }
}

/// A refusal: its status and code, and nothing written on `(flow, trig)`.
async fn refused(
    fx: &Fx,
    flow: Uuid,
    trig: &str,
    body: Value,
    status: StatusCode,
    code: &str,
) -> String {
    let before = fx.rows(flow, trig).await;
    let (got, res) = fx.create(flow, trig, body.clone()).await;
    assert_eq!(got, status, "{body} -> {res}");
    assert_eq!(res["code"], json!(code), "{body} -> {res}");
    assert_eq!(fx.rows(flow, trig).await, before, "{body} wrote something");
    res["error"].as_str().unwrap_or_default().to_string()
}

/// Test 1: a 5-tick window ending a minute ago, with inputs and labels in
/// map form: 201 with the entry, the trigger moved to the first tick, the old
/// date kept, one audit event naming keys and no values; then five passes
/// fire the five ticks, each with the backfill's id, inputs and labels.
#[tokio::test]
async fn a_created_backfill_moves_the_trigger_and_its_window_fires() {
    let Some(fx) = fx().await else { return };
    fx.quiet_the_queue().await;
    // The planner refuses a schedule that does not supply every input with
    // no default (`schedule_missing_inputs`), so this one supplies `req`.
    let flow = fx
        .flow_src(&src_inputs(
            "create",
            "  - id: region\n    type: STRING\n    defaults: us\n  - id: req\n    type: STRING\n",
            &cron("s", "*/1 * * * *", "    inputs:\n      req: yaml\n"),
        ))
        .await;
    let old = fx.next_date(flow, "s").await.unwrap();
    let m = fx.minute().await;
    let (start, end) = (m - Duration::minutes(5), m - Duration::minutes(1));

    let (status, e) = fx
        .create(
            flow,
            "s",
            json!({
                "start": rfc(start), "end": rfc(end),
                "inputs": {"region": "eu", "req": "yes"},
                "labels": {"team": "ops"},
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{e}");
    assert_eq!(e["trigger_id"], json!("s"));
    let b = &e["backfill"];
    assert_eq!(b["total_ticks"], json!(5), "{e}");
    assert_eq!(b["ticks_done"], json!(0));
    assert_eq!(ts(&b["current_tick"]), start);
    assert_eq!(ts(&b["start"]), start);
    assert_eq!(ts(&b["end"]), end);
    assert_eq!(b["labels"], json!([{"key": "team", "value": "ops"}]));
    assert_eq!(b["inputs"], json!({"region": "eu", "req": "yes"}));
    assert_eq!(b["paused"], json!(false));
    assert_eq!(e["backfill_refusal"], json!("running"));
    assert_eq!(fx.next_date(flow, "s").await, Some(start));
    let row = common::backfill_row(&fx.db, flow, "s").await.unwrap();
    assert_eq!(ts(&row["previous_next_evaluation_date"]), old);
    assert_eq!(row["created_by"], json!(fx.user.0));

    let audit = common::audit_mutations(&fx.app);
    assert_eq!(audit.len(), 1, "{audit:?}");
    let (action, resource, detail) = &audit[0];
    assert_eq!(action, "workflow_trigger:backfill_create");
    assert_eq!(resource.as_deref(), Some(format!("{flow}/s").as_str()));
    assert_eq!(detail["total_ticks"], json!(5));
    assert_eq!(detail["input_keys"], json!(["region", "req"]));
    assert_eq!(detail["label_keys"], json!(["team"]));
    assert_eq!(ts(&detail["start"]), start);
    assert_eq!(ts(&detail["end"]), end);
    let text = detail.to_string();
    assert!(
        !text.contains("ops") && !text.contains("eu") && !text.contains("yes"),
        "values leaked into the audit: {text}"
    );

    for i in 0..5 {
        let r = fx.pass().await;
        let why: Option<String> = sqlx::query_scalar(
            "SELECT last_error FROM workflow.triggers WHERE flow_id = $1 AND trigger_id = 's'",
        )
        .bind(flow)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
        assert_eq!(r.fired, 1, "pass {i}: {r:?}, last_error {why:?}");
    }
    let dates = common::scheduled_dates(&fx.db, flow).await;
    assert_eq!(
        dates,
        (0..5)
            .map(|i| start + Duration::minutes(i))
            .collect::<Vec<_>>()
    );
    let fired: Vec<(Option<Uuid>, Value, Option<Value>)> = sqlx::query_as(
        "SELECT backfill_id, inputs, labels FROM workflow.executions WHERE flow_id = $1",
    )
    .bind(flow)
    .fetch_all(&fx.db.pool)
    .await
    .unwrap();
    let id: Uuid = serde_json::from_value(b["id"].clone()).unwrap();
    for (bf, inputs, labels) in fired {
        assert_eq!(bf, Some(id));
        assert_eq!(inputs["region"], json!("eu"));
        assert_eq!(labels, Some(json!([{"key": "team", "value": "ops"}])));
    }
    assert_eq!(common::backfill_row(&fx.db, flow, "s").await, None);
}

/// Test 2: the window is inclusive at its start (D4): a start exactly on a
/// tick is the first tick; a start between ticks rounds up.
#[tokio::test]
async fn a_start_on_a_tick_is_the_first_tick() {
    let Some(fx) = fx().await else { return };
    let flow = fx
        .flow(
            "incl",
            &format!("{}{}", schedule("on"), schedule("between")),
        )
        .await;
    let m = fx.minute().await;
    let start = m - Duration::minutes(3);
    let (status, e) = fx
        .create(
            flow,
            "on",
            json!({"start": rfc(start), "end": rfc(m - Duration::minutes(1))}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{e}");
    assert_eq!(ts(&e["backfill"]["current_tick"]), start);
    assert_eq!(e["backfill"]["total_ticks"], json!(3));

    let (status, e) = fx
        .create(
            flow,
            "between",
            json!({"start": rfc(start + Duration::seconds(30)), "end": rfc(m - Duration::minutes(1))}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{e}");
    assert_eq!(
        ts(&e["backfill"]["current_tick"]),
        start + Duration::minutes(1)
    );
    assert_eq!(e["backfill"]["total_ticks"], json!(2));
}

/// Test 3, the window: `start >= end`, an end in the future (the message
/// gives the server's time), a start 121 months back, a window with no tick,
/// 10,001 ticks -- and exactly 10,000 accepted; an absent `end` is now.
#[tokio::test]
async fn create_refuses_a_window_it_cannot_run() {
    let Some(fx) = fx().await else { return };
    let flow = fx
        .flow(
            "window",
            &format!(
                "{}{}{}{}",
                schedule("s"),
                cron("d", "0 2 * * *", ""),
                cron("sec", "* * * * * *", "    withSeconds: true\n"),
                schedule("open")
            ),
        )
        .await;
    let m = fx.minute().await;
    let now = fx.now().await;
    let a = m - Duration::minutes(10);

    refused(
        &fx,
        flow,
        "s",
        json!({"start": rfc(a), "end": rfc(a)}),
        StatusCode::BAD_REQUEST,
        "backfill_window_invalid",
    )
    .await;
    refused(
        &fx,
        flow,
        "s",
        json!({"start": rfc(a), "end": rfc(a - Duration::minutes(1))}),
        StatusCode::BAD_REQUEST,
        "backfill_window_invalid",
    )
    .await;
    let msg = refused(
        &fx,
        flow,
        "s",
        json!({"start": rfc(a), "end": rfc(now + Duration::hours(1))}),
        StatusCode::BAD_REQUEST,
        "backfill_end_in_future",
    )
    .await;
    assert!(
        msg.contains(&now.format("%Y-%m-%dT%H").to_string()) && msg.contains("server"),
        "the message gives the server's time: {msg}"
    );
    let early = now.checked_sub_months(chrono::Months::new(121)).unwrap();
    refused(
        &fx,
        flow,
        "s",
        json!({"start": rfc(early), "end": rfc(early + Duration::hours(1))}),
        StatusCode::BAD_REQUEST,
        "backfill_start_too_early",
    )
    .await;
    // `0 2 * * *` over [03:00, 04:00] of yesterday, in UTC.
    let day = m - Duration::days(1);
    let three = day.date_naive().and_hms_opt(3, 0, 0).unwrap().and_utc();
    refused(
        &fx,
        flow,
        "d",
        json!({"start": rfc(three), "end": rfc(three + Duration::hours(1))}),
        StatusCode::BAD_REQUEST,
        "backfill_no_ticks",
    )
    .await;
    // 10,001 seconds inclusive, then 10,000.
    refused(
        &fx,
        flow,
        "sec",
        json!({"start": rfc(m - Duration::seconds(10_000)), "end": rfc(m)}),
        StatusCode::BAD_REQUEST,
        "backfill_too_many_ticks",
    )
    .await;
    let (status, e) = fx
        .create(
            flow,
            "sec",
            json!({"start": rfc(m - Duration::seconds(9_999)), "end": rfc(m)}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{e}");
    assert_eq!(e["backfill"]["total_ticks"], json!(10_000));

    // No `end`: the transaction's `now()`.
    let (status, e) = fx.create(flow, "open", json!({"start": rfc(a)})).await;
    assert_eq!(status, StatusCode::CREATED, "{e}");
    let end = ts(&e["backfill"]["end"]);
    assert!(
        end >= now && end <= fx.now().await,
        "{end} is the server's now"
    );
}

/// Test 3, inputs and labels: `inputs` not an object, a FILE input named, a
/// templated schedule input not overridden (and overridden: accepted), a
/// required input missing (and present: accepted); each label rule.
#[tokio::test]
async fn create_refuses_inputs_and_labels_it_cannot_run() {
    let Some(fx) = fx().await else { return };
    let triggers = format!(
        "{}{}",
        cron("s", "*/1 * * * *", ""),
        cron(
            "tpl",
            "*/1 * * * *",
            "    inputs:\n      region: \"{{ trigger.date }}\"\n"
        )
    );
    let flow = fx.flow_src(&src_inputs("inputs", INPUTS, &triggers)).await;
    let m = fx.minute().await;
    let w = |extra: Value| {
        let mut b =
            json!({"start": rfc(m - Duration::minutes(5)), "end": rfc(m - Duration::minutes(1))});
        b.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        b
    };
    const BAD: StatusCode = StatusCode::BAD_REQUEST;

    let msg = refused(
        &fx,
        flow,
        "s",
        w(json!({"inputs": [], "labels": []})),
        BAD,
        "backfill_inputs_invalid",
    )
    .await;
    assert!(msg.contains("object"), "{msg}");
    let msg = refused(
        &fx,
        flow,
        "s",
        w(json!({"inputs": {"req": "x", "upload": "kestra:///a"}})),
        BAD,
        "backfill_inputs_invalid",
    )
    .await;
    assert!(msg.contains("upload"), "{msg}");
    let msg = refused(
        &fx,
        flow,
        "tpl",
        w(json!({"inputs": {"req": "x"}})),
        BAD,
        "backfill_inputs_invalid",
    )
    .await;
    assert!(msg.contains("region"), "{msg}");
    let msg = refused(
        &fx,
        flow,
        "s",
        w(json!({"inputs": {"region": "eu"}})),
        BAD,
        "backfill_inputs_invalid",
    )
    .await;
    assert!(msg.contains("req"), "{msg}");
    let msg = refused(&fx, flow, "s", w(json!({})), BAD, "backfill_inputs_invalid").await;
    assert!(msg.contains("req"), "{msg}");

    let ok = json!({"inputs": {"req": "x"}});
    for labels in [
        json!({"system.x": "v"}),
        json!([{"key": "k", "value": ""}]),
        json!({"k": "a {{ b }}"}),
        Value::Object((0..51).map(|i| (format!("k{i}"), json!("v"))).collect()),
    ] {
        let mut body = w(ok.clone());
        body["labels"] = labels;
        refused(&fx, flow, "s", body, BAD, "backfill_labels_invalid").await;
    }

    // Overridden, the templated input is accepted; present, the required one.
    let (status, e) = fx
        .create(
            flow,
            "tpl",
            w(json!({"inputs": {"req": "x", "region": "eu"}})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{e}");
    let (status, e) = fx
        .create(
            flow,
            "s",
            w(json!({"inputs": {"req": "x"}, "labels": [{"key": "reason", "value": "r"}]})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{e}");
    assert_eq!(
        e["backfill"]["labels"],
        json!([{"key": "reason", "value": "r"}])
    );
    // A second create: 409, nothing written.
    refused(
        &fx,
        flow,
        "s",
        w(ok),
        StatusCode::CONFLICT,
        "backfill_exists",
    )
    .await;
}

/// Review M7: the start-time input check runs at create, on the schedule's
/// `inputs:` overlaid by the backfill's, so a value every tick would refuse
/// is refused here -- not at the first tick, which would end the backfill.
/// The text is the start check's own.
#[tokio::test]
async fn create_refuses_an_input_value_its_ticks_would_refuse() {
    let Some(fx) = fx().await else { return };
    let inputs = "  - id: n\n    type: INT\n    min: 1\n";
    let triggers = format!(
        "{}{}",
        cron("s", "*/1 * * * *", ""),
        cron("bad", "*/1 * * * *", "    inputs:\n      n: abc\n")
    );
    let flow = fx.flow_src(&src_inputs("checked", inputs, &triggers)).await;
    let m = fx.minute().await;
    let w = |extra: Value| {
        let mut b =
            json!({"start": rfc(m - Duration::minutes(5)), "end": rfc(m - Duration::minutes(1))});
        b.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        b
    };
    const BAD: StatusCode = StatusCode::BAD_REQUEST;

    // The backfill's own value.
    let msg = refused(
        &fx,
        flow,
        "s",
        w(json!({"inputs": {"n": 0}})),
        BAD,
        "backfill_inputs_invalid",
    )
    .await;
    assert!(
        msg.contains("Invalid value for input `n`. Cause: it must be more than `1`"),
        "{msg}"
    );
    // The schedule's own value, not overridden.
    let msg = refused(
        &fx,
        flow,
        "bad",
        w(json!({})),
        BAD,
        "backfill_inputs_invalid",
    )
    .await;
    assert!(
        msg.contains("Invalid value for input `n`. Cause: Cannot convert value 'abc' to Integer"),
        "{msg}"
    );

    // Overridden by a good value, the schedule's bad one is accepted.
    let (status, e) = fx.create(flow, "bad", w(json!({"inputs": {"n": 3}}))).await;
    assert_eq!(status, StatusCode::CREATED, "{e}");
    let (status, e) = fx.create(flow, "s", w(json!({"inputs": {"n": "2"}}))).await;
    assert_eq!(status, StatusCode::CREATED, "{e}");
}

/// Test 3, the trigger's state: a webhook, a YAML-disabled trigger, a
/// switched-off one, a `disabled` flow, a schedule off the queue (its
/// `last_error` in the message), an unknown id, another workspace's flow, a
/// trashed flow, a source that does not parse.
#[tokio::test]
async fn create_refuses_a_trigger_that_cannot_backfill() {
    let Some(fx) = fx().await else { return };
    let flow = fx
        .flow(
            "state",
            &format!(
                "{}{}{}{}  - id: w\n    type: {WEBHOOK}\n    key: k\n",
                schedule("s"),
                schedule("switched"),
                schedule("offq"),
                cron("y", "*/1 * * * *", "    disabled: true\n"),
            ),
        )
        .await;
    let m = fx.minute().await;
    let body =
        json!({"start": rfc(m - Duration::minutes(5)), "end": rfc(m - Duration::minutes(1))});
    const CONFLICT: StatusCode = StatusCode::CONFLICT;

    refused(
        &fx,
        flow,
        "w",
        body.clone(),
        CONFLICT,
        "backfill_not_supported",
    )
    .await;
    refused(&fx, flow, "y", body.clone(), CONFLICT, "trigger_disabled").await;
    common::switch_off(&fx.db, flow, "switched").await;
    refused(
        &fx,
        flow,
        "switched",
        body.clone(),
        CONFLICT,
        "trigger_disabled",
    )
    .await;
    sqlx::query(
        "UPDATE workflow.triggers SET next_evaluation_date = NULL, last_error = 'the cron broke' \
          WHERE flow_id = $1 AND trigger_id = 'offq'",
    )
    .bind(flow)
    .execute(&fx.db.pool)
    .await
    .unwrap();
    let msg = refused(
        &fx,
        flow,
        "offq",
        body.clone(),
        CONFLICT,
        "trigger_off_queue",
    )
    .await;
    assert!(msg.contains("the cron broke"), "{msg}");
    refused(
        &fx,
        flow,
        "nope",
        body.clone(),
        StatusCode::NOT_FOUND,
        "not_found",
    )
    .await;

    let disabled = common::seed_flow(
        &fx.db,
        fx.ws,
        fx.user,
        &src("dis", &schedule("s")),
        "disabled",
    )
    .await;
    refused(
        &fx,
        disabled,
        "s",
        body.clone(),
        CONFLICT,
        "trigger_disabled",
    )
    .await;

    let (other_ws, other_user) = common::seed_workspace(&fx.db).await;
    let foreign = common::seed_flow(
        &fx.db,
        other_ws,
        other_user,
        &src("foreign", &schedule("s")),
        "draft",
    )
    .await;
    refused(
        &fx,
        foreign,
        "s",
        body.clone(),
        StatusCode::NOT_FOUND,
        "not_found",
    )
    .await;

    let trashed = fx.flow("trashed", &schedule("s")).await;
    fx.sql(
        "UPDATE workflow.flows SET deleted_at = now() WHERE id = $1",
        trashed,
    )
    .await;
    refused(
        &fx,
        trashed,
        "s",
        body.clone(),
        StatusCode::NOT_FOUND,
        "not_found",
    )
    .await;

    let broken = fx.flow("broken", &schedule("s")).await;
    fx.sql(
        "UPDATE workflow.flows SET source = 'id: [unclosed' WHERE id = $1",
        broken,
    )
    .await;
    refused(
        &fx,
        broken,
        "s",
        body.clone(),
        StatusCode::BAD_REQUEST,
        "source_does_not_parse",
    )
    .await;

    // The healthy trigger still accepts, so the refusals above were theirs.
    let (status, e) = fx.create(flow, "s", body).await;
    assert_eq!(status, StatusCode::CREATED, "{e}");
    assert!(
        common::audit_mutations(&fx.app).len() == 1,
        "refusals are not audited"
    );
}

/// Test 3, access and body: `workflow:read` 403, no session 401, bad JSON
/// 400 `invalid_body`, 65 KiB 413 `body_too_large`; nothing written.
#[tokio::test]
async fn create_refuses_a_caller_or_a_body() {
    let Some(mut fx) = fx().await else { return };
    let flow = fx.flow("access", &schedule("s")).await;
    let reader = fx.user("Reader", &["workflow:read"]).await;
    fx.rebuild_app().await;
    let m = fx.minute().await;
    let body =
        json!({"start": rfc(m - Duration::minutes(5)), "end": rfc(m - Duration::minutes(1))});
    let before = fx.rows(flow, "s").await;
    let path = fx.bf_path(flow, "s");

    let (status, res) = fx.create_as(reader, flow, "s", body.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{res}");
    let res = common::anon_post(&fx.app, &path).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let res = common::send_raw(
        &fx.app,
        "POST",
        &path,
        fx.user,
        "application/json",
        b"{".to_vec(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(common::json(res).await["code"], json!("invalid_body"));
    // A reader with a bad body: the permission is checked first.
    let res = common::send_raw(
        &fx.app,
        "POST",
        &path,
        reader,
        "application/json",
        b"{".to_vec(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let mut big = body.clone();
    big["labels"] = json!({"k": "v".repeat(65 * 1024)});
    let res = common::send_raw(
        &fx.app,
        "POST",
        &path,
        fx.user,
        "application/json",
        big.to_string().into_bytes(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(common::json(res).await["code"], json!("body_too_large"));
    assert_eq!(fx.rows(flow, "s").await, before);
    assert!(common::audit_mutations(&fx.app).is_empty());
}

/// Test 4: pause, pause again, resume; the trigger row and the resume
/// position untouched (D5); a resumed backfill fires its `current_tick`;
/// `no_backfill`; a switched-off trigger's backfill can be paused.
#[tokio::test]
async fn pause_and_resume_write_only_the_backfill() {
    let Some(fx) = fx().await else { return };
    fx.quiet_the_queue().await;
    let flow = fx
        .flow(
            "pause",
            &format!(
                "{}{}{}",
                schedule("s"),
                schedule("none"),
                schedule("frozen")
            ),
        )
        .await;
    let m = fx.minute().await;
    let start = m - Duration::minutes(10);
    let body = json!({"start": rfc(start), "end": rfc(m - Duration::minutes(5))});
    let (status, _) = fx.create(flow, "s", body.clone()).await;
    assert_eq!(status, StatusCode::CREATED);
    common::audit_mutations(&fx.app);
    let (trigger_before, bf_before) = fx.rows(flow, "s").await;

    let (status, e) = fx.pause(flow, "s", true).await;
    assert_eq!(status, StatusCode::OK, "{e}");
    assert_eq!(e["backfill"]["paused"], json!(true));
    assert_eq!(e["backfill"]["paused_by_name"], json!("t"));
    assert!(e["backfill"]["paused_at"].is_string());
    let (trigger_after, bf_after) = fx.rows(flow, "s").await;
    assert_eq!(
        trigger_after, trigger_before,
        "pause writes no trigger date"
    );
    let (bf_before, bf_after) = (bf_before.unwrap(), bf_after.unwrap());
    assert_eq!(
        bf_after["previous_next_evaluation_date"],
        bf_before["previous_next_evaluation_date"]
    );
    assert_eq!(bf_after["current_tick"], bf_before["current_tick"]);
    assert_eq!(bf_after["paused_by"], json!(fx.user.0));
    let (status, e) = fx.pause(flow, "s", true).await;
    assert_eq!(status, StatusCode::OK, "{e}");
    assert_eq!(
        fx.rows(flow, "s").await.1.unwrap(),
        bf_after,
        "no second write"
    );
    let audit = common::audit_mutations(&fx.app);
    assert_eq!(
        audit.iter().map(|a| a.0.as_str()).collect::<Vec<_>>(),
        vec!["workflow_trigger:backfill_pause"]
    );
    assert_eq!(fx.pass().await.fired, 0, "a paused backfill does not fire");

    let (status, e) = fx.pause(flow, "s", false).await;
    assert_eq!(status, StatusCode::OK, "{e}");
    assert_eq!(e["backfill"]["paused"], json!(false));
    assert_eq!(e["backfill"]["paused_by_name"], Value::Null);
    let row = fx.rows(flow, "s").await.1.unwrap();
    assert_eq!(row["paused_by"], Value::Null);
    assert_eq!(row["paused_at"], Value::Null);
    let audit = common::audit_mutations(&fx.app);
    assert_eq!(
        audit.iter().map(|a| a.0.as_str()).collect::<Vec<_>>(),
        vec!["workflow_trigger:backfill_resume"]
    );
    assert_eq!(fx.pass().await.fired, 1);
    assert_eq!(common::scheduled_dates(&fx.db, flow).await, vec![start]);

    let (status, e) = fx.pause(flow, "none", true).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{e}");
    assert_eq!(e["code"], json!("no_backfill"));

    let (status, _) = fx.create(flow, "frozen", body).await;
    assert_eq!(status, StatusCode::CREATED);
    common::switch_off(&fx.db, flow, "frozen").await;
    let (status, e) = fx.pause(flow, "frozen", true).await;
    assert_eq!(status, StatusCode::OK, "{e}");
    assert_eq!(e["backfill"]["paused"], json!(true));
    assert_eq!(e["runtime_disabled"], json!(true));
}

/// Test 5: delete resumes the regular schedule from where it stood: a future
/// resume position is the next date; **Kestra's delete bug** (pause, resume,
/// delete) resumes from the stored position, never a window date; `NONE`
/// with a past position: the first tick after now; `no_backfill`; the
/// executions already fired stay.
#[tokio::test]
async fn delete_resumes_the_regular_schedule_where_it_stood() {
    let Some(fx) = fx().await else { return };
    fx.quiet_the_queue().await;
    let flow = fx
        .flow(
            "delete",
            &format!(
                "{}{}{}",
                schedule("s"),
                schedule("bug"),
                cron("none", "*/1 * * * *", "    recoverMissedSchedules: NONE\n")
            ),
        )
        .await;
    // Due first, so `make_due` waits out a minute's last ten seconds before
    // anything below reads the clock.
    common::make_due(&fx.db, flow, "none", 3).await;
    let m = fx.minute().await;
    let body =
        json!({"start": rfc(m - Duration::minutes(10)), "end": rfc(m - Duration::minutes(5))});

    // A future resume position; one window tick fired first. Set here, after
    // `make_due`, as for `bug` below: the date the save stored is the minute
    // after the SEED, and a seed in a minute's last ten seconds has
    // `make_due` wait across that minute, which leaves it in the past.
    fx.sql("UPDATE workflow.triggers SET next_evaluation_date = date_trunc('minute', now()) + interval '1 minute' WHERE flow_id = $1 AND trigger_id = 's'", flow).await;
    let prev = fx.next_date(flow, "s").await.unwrap();
    assert!(prev > fx.now().await);
    let (status, _) = fx.create(flow, "s", body.clone()).await;
    assert_eq!(status, StatusCode::CREATED);
    fx.sql("UPDATE workflow.triggers SET next_evaluation_date = NULL WHERE flow_id = $1 AND trigger_id <> 's'", flow).await;
    assert_eq!(fx.pass().await.fired, 1);
    common::audit_mutations(&fx.app);
    let (status, e) = fx.delete_bf(flow, "s").await;
    assert_eq!(status, StatusCode::OK, "{e}");
    assert_eq!(e["backfill"], Value::Null);
    assert_eq!(e["can_backfill"], json!(true));
    assert_eq!(fx.next_date(flow, "s").await, Some(prev));
    assert_eq!(common::backfill_row(&fx.db, flow, "s").await, None);
    assert_eq!(
        common::scheduled_dates(&fx.db, flow).await.len(),
        1,
        "fired executions stay"
    );
    let audit = common::audit_mutations(&fx.app);
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0].0, "workflow_trigger:backfill_delete");
    assert_eq!(ts(&audit[0].2["resumed_at"]), prev);

    // Kestra's bug: pause, resume, delete -- the stored position, not a window date.
    fx.sql("UPDATE workflow.triggers SET next_evaluation_date = date_trunc('minute', now()) + interval '1 minute' WHERE flow_id = $1 AND trigger_id = 'bug'", flow).await;
    let prev = fx.next_date(flow, "bug").await.unwrap();
    let (status, _) = fx.create(flow, "bug", body.clone()).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(fx.pause(flow, "bug", true).await.0, StatusCode::OK);
    assert_eq!(fx.pause(flow, "bug", false).await.0, StatusCode::OK);
    let (status, e) = fx.delete_bf(flow, "bug").await;
    assert_eq!(status, StatusCode::OK, "{e}");
    assert_eq!(
        fx.next_date(flow, "bug").await,
        Some(prev),
        "the stored position"
    );

    // `NONE` with a past position: the first tick after now.
    fx.sql("UPDATE workflow.triggers SET next_evaluation_date = date_trunc('minute', now()) - interval '2 minutes' WHERE flow_id = $1 AND trigger_id = 'none'", flow).await;
    let (status, _) = fx.create(flow, "none", body).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, e) = fx.delete_bf(flow, "none").await;
    assert_eq!(status, StatusCode::OK, "{e}");
    assert_eq!(
        fx.next_date(flow, "none").await,
        Some(fx.minute().await + Duration::minutes(1))
    );

    let (status, e) = fx.delete_bf(flow, "none").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{e}");
    assert_eq!(e["code"], json!("no_backfill"));
}

/// Test 6: bulk pause and bulk delete -- 100 entries accepted, 101 refused
/// with nothing written, a mix answered in order with its codes.
#[tokio::test]
async fn bulk_pause_and_delete_answer_each_entry() {
    let Some(fx) = fx().await else { return };
    let flow = fx
        .flow("bulk", &format!("{}{}", schedule("s"), schedule("none")))
        .await;
    let (other_ws, other_user) = common::seed_workspace(&fx.db).await;
    let foreign = common::seed_flow(
        &fx.db,
        other_ws,
        other_user,
        &src("bulkf", &schedule("s")),
        "draft",
    )
    .await;
    let m = fx.minute().await;
    let (status, _) = fx
        .create(
            flow,
            "s",
            json!({"start": rfc(m - Duration::minutes(5)), "end": rfc(m - Duration::minutes(1))}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    common::audit_mutations(&fx.app);
    let r = |f: Uuid, t: &str| json!({"flow_id": f, "trigger_id": t});
    let before = fx.rows(flow, "s").await;

    let refs: Vec<Value> = (0..101).map(|_| r(flow, "s")).collect();
    let (status, res) = fx
        .bulk("PUT", "paused", json!({"triggers": refs, "paused": true}))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert_eq!(res["code"], json!("too_many_triggers"));
    let (status, res) = fx.bulk("POST", "delete", json!({"triggers": refs})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert_eq!(res["code"], json!("too_many_triggers"));
    assert_eq!(fx.rows(flow, "s").await, before, "nothing written");

    let refs: Vec<Value> = (0..100).map(|_| r(flow, "none")).collect();
    let (status, res) = fx
        .bulk("PUT", "paused", json!({"triggers": refs, "paused": true}))
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["results"].as_array().unwrap().len(), 100);

    let mix = json!([r(flow, "s"), r(flow, "none"), r(foreign, "s")]);
    let (status, res) = fx
        .bulk("PUT", "paused", json!({"triggers": mix, "paused": true}))
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let results = res["results"].as_array().unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0]["ok"], json!(true));
    assert_eq!(results[0]["trigger"]["backfill"]["paused"], json!(true));
    assert_eq!(results[1]["ok"], json!(false));
    assert_eq!(results[1]["code"], json!("no_backfill"));
    assert_eq!(results[2]["ok"], json!(false));
    assert_eq!(results[2]["code"], json!("not_found"));
    assert_eq!(results[2]["flow_id"], json!(foreign));
    assert_eq!(common::audit_mutations(&fx.app).len(), 1);

    let (status, res) = fx.bulk("POST", "delete", json!({"triggers": mix})).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let results = res["results"].as_array().unwrap();
    assert_eq!(results[0]["ok"], json!(true), "{res}");
    assert_eq!(results[0]["trigger"]["backfill"], Value::Null);
    assert_eq!(results[1]["code"], json!("no_backfill"));
    assert_eq!(results[2]["code"], json!("not_found"));
    assert_eq!(common::backfill_row(&fx.db, flow, "s").await, None);
    let audit = common::audit_mutations(&fx.app);
    assert_eq!(
        audit.iter().map(|a| a.0.as_str()).collect::<Vec<_>>(),
        vec!["workflow_trigger:backfill_delete"]
    );
}
