//! W7f Task 6: reading backfills (spec §3.9, D12, D15) -- the `backfill`,
//! `can_backfill` and `backfill_refusal` fields of a trigger entry, the
//! workspace list's `backfill` filter, `missed_ticks` during a backfill --
//! and the fire after a re-enable during a backfill (§3.8, D11). Task 7 adds
//! the write routes. Requires DATABASE_URL.
//!
//! **The tests in this binary run one at a time** (`SERIAL`): the re-enable
//! test runs a `fire_due` pass, which is global, so it first takes every
//! trigger an earlier test left behind off the queue (`tests/backfill_fire.rs`'s
//! reason), and no other test of this binary may be running then.
//!
//! A backfill is staged by SQL (`common::stage_backfill`) until Task 7's
//! create route exists. Window ticks are built from the database clock and
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
