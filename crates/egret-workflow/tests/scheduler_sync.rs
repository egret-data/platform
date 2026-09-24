//! W12 Task 2: `workflow.triggers` is kept in step with the flow YAML by
//! `scheduler::sync`, called inside every `FlowRepo` write that can change what
//! fires. Requires DATABASE_URL.
//!
//! Every test goes through `FlowRepo`, never through SQL writes to
//! `workflow.flows`, because what is under test is that the WRITERS call
//! `sync` -- a test that inserted flows by hand and then called `sync` itself
//! would pass with every writer forgetting it. The only SQL writes here are to
//! `workflow.triggers` itself: the sentinel date that proves a later save did,
//! or did not, touch the schedule.
//!
//! See docs/en/specs/2026-09-24-workflow-w12-trigger-loop-design.md §3.3.

mod common;

use chrono::{DateTime, Duration, NaiveTime, TimeZone, Timelike, Utc};
use egret_core::UserId;
use egret_metastore::Db;
use egret_workflow::models::TriggerKind;
use egret_workflow::{CreateFlowInput, FlowRepo, FlowRow, UpdateFlowInput};
use uuid::Uuid;

const CRON_A: &str = "id: f\nnamespace: t\ntriggers:\n  - id: s\n    type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"0 2 * * *\"\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";

/// A flow with the given `triggers:` block and flow-level prelude.
fn flow_yaml(prelude: &str, triggers: &str, message: &str) -> String {
    format!(
        "id: f\nnamespace: t\n{prelude}triggers:\n{triggers}tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: {message}\n"
    )
}

fn schedule(id: &str, cron: &str, extra: &str) -> String {
    format!(
        "  - id: {id}\n    type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"{cron}\"\n{extra}"
    )
}

/// The value no `sync` would ever compute: a date in the past. If it survives
/// a save, that save did not re-initialise the schedule.
fn sentinel() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0).unwrap()
}

type TriggerRow = (Option<DateTime<Utc>>, bool, Option<String>);

async fn row(db: &Db, flow: Uuid, trig: &str) -> Option<TriggerRow> {
    sqlx::query_as(
        "SELECT next_evaluation_date, disabled, last_error FROM workflow.triggers \
         WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(flow)
    .bind(trig)
    .fetch_optional(&db.pool)
    .await
    .unwrap()
}

async fn trigger_ids(db: &Db, flow: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT trigger_id FROM workflow.triggers WHERE flow_id = $1 ORDER BY trigger_id",
    )
    .bind(flow)
    .fetch_all(&db.pool)
    .await
    .unwrap()
}

async fn set_sentinel(db: &Db, flow: Uuid, trig: &str) {
    let n = sqlx::query(
        "UPDATE workflow.triggers SET next_evaluation_date = $3, evaluated_at = $3 \
         WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(flow)
    .bind(trig)
    .bind(sentinel())
    .execute(&db.pool)
    .await
    .unwrap()
    .rows_affected();
    assert_eq!(n, 1, "the sentinel must land on exactly one row");
}

async fn db_now(db: &Db) -> DateTime<Utc> {
    sqlx::query_scalar("SELECT now()")
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

/// The first `hh:00:00` UTC strictly after `at`.
fn next_at_hour(at: DateTime<Utc>, hour: u32) -> DateTime<Utc> {
    let today = at
        .date_naive()
        .and_time(NaiveTime::from_hms_opt(hour, 0, 0).unwrap())
        .and_utc();
    if today > at {
        today
    } else {
        today + Duration::days(1)
    }
}

/// Parsing is asserted before every write, so that "no row" can never be the
/// silent consequence of a YAML typo (`sync` treats an unparseable source as
/// declaring nothing).
fn parses(src: &str) -> egret_workflow::models::Flow {
    egret_workflow::spec::parse(src).unwrap_or_else(|e| panic!("fixture must parse: {e:?}\n{src}"))
}

struct Fx {
    db: Db,
    repo: FlowRepo,
    ws: Uuid,
    user: UserId,
}

async fn fx() -> Option<Fx> {
    let db = common::db().await?;
    let (ws, user) = common::seed_workspace(&db).await;
    let repo = FlowRepo::new(db.clone());
    Some(Fx { db, repo, ws, user })
}

impl Fx {
    async fn create(&self, src: &str) -> FlowRow {
        parses(src);
        self.repo
            .create(
                self.ws,
                self.user,
                &CreateFlowInput {
                    namespace: "t".into(),
                    flow_id: "f".into(),
                    source: src.into(),
                    plan: serde_json::json!({}),
                    folder_id: None,
                },
            )
            .await
            .expect("create")
    }

    async fn update(&self, id: Uuid, src: &str) -> FlowRow {
        parses(src);
        self.repo
            .update(
                self.ws,
                id,
                self.user,
                &UpdateFlowInput {
                    source: Some(src.into()),
                    plan: Some(serde_json::json!({})),
                    ..Default::default()
                },
            )
            .await
            .expect("update")
    }
}

#[tokio::test]
async fn create_writes_one_row_per_schedule_with_the_next_tick() {
    let Some(fx) = fx().await else { return };
    let before = db_now(&fx.db).await;
    let flow = fx.create(CRON_A).await;
    let after = db_now(&fx.db).await;

    let (next, disabled, err) = row(&fx.db, flow.id, "s").await.expect("a row for `s`");
    let next = next.expect("on the queue");
    assert!(
        next > before,
        "the NEXT tick, never now: {next} vs {before}"
    );
    assert!(
        next <= after + Duration::days(1),
        "{next} is more than a day out"
    );
    assert_eq!(
        (next.hour(), next.minute(), next.second(), next.nanosecond()),
        (2, 0, 0, 0),
        "`0 2 * * *` in UTC: {next}"
    );
    // Either reading of the DB clock gives the same first 02:00, unless the
    // create straddled one -- then it is the later of the two.
    assert!(
        next == next_at_hour(before, 2) || next == next_at_hour(after, 2),
        "{next} is not the first 02:00 after the save"
    );
    assert!(!disabled);
    assert_eq!(err, None);

    let (ws, key, evaluated): (Uuid, String, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT workspace_id, spec_key, evaluated_at FROM workflow.triggers \
         WHERE flow_id = $1 AND trigger_id = 's'",
    )
    .bind(flow.id)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(ws, fx.ws);
    assert_eq!(key, "0 2 * * *|false|");
    assert_eq!(evaluated, None);
    assert_eq!(trigger_ids(&fx.db, flow.id).await, vec!["s".to_string()]);
}

#[tokio::test]
async fn editing_an_unrelated_task_keeps_the_schedule() {
    let Some(fx) = fx().await else { return };
    let flow = fx.create(CRON_A).await;
    set_sentinel(&fx.db, flow.id, "s").await;

    let edited = CRON_A.replace("message: hi", "message: changed");
    assert_ne!(edited, CRON_A, "the edit must change the source");
    fx.update(flow.id, &edited).await;

    let (next, _, _) = row(&fx.db, flow.id, "s").await.expect("row kept");
    assert_eq!(
        next,
        Some(sentinel()),
        "editing a task must not reset the schedule"
    );
    let evaluated: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT evaluated_at FROM workflow.triggers WHERE flow_id = $1 AND trigger_id = 's'",
    )
    .bind(flow.id)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(evaluated, Some(sentinel()), "evaluated_at kept too");
}

#[tokio::test]
async fn changing_the_cron_reinitialises() {
    let Some(fx) = fx().await else { return };
    let flow = fx.create(CRON_A).await;
    set_sentinel(&fx.db, flow.id, "s").await;

    let before = db_now(&fx.db).await;
    fx.update(flow.id, &CRON_A.replace("0 2 * * *", "0 3 * * *"))
        .await;

    let (next, _, err) = row(&fx.db, flow.id, "s").await.expect("row kept");
    let next = next.expect("still on the queue");
    assert_ne!(next, sentinel(), "a changed cron must re-initialise");
    assert!(next > before);
    assert_eq!((next.hour(), next.minute(), next.second()), (3, 0, 0));
    assert_eq!(err, None);
    let (key, evaluated): (String, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT spec_key, evaluated_at FROM workflow.triggers \
         WHERE flow_id = $1 AND trigger_id = 's'",
    )
    .bind(flow.id)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(key, "0 3 * * *|false|");
    assert_eq!(evaluated, None, "re-initialising clears evaluated_at");
}

#[tokio::test]
async fn removing_a_schedule_deletes_its_row() {
    let Some(fx) = fx().await else { return };
    let two = flow_yaml(
        "",
        &(schedule("s", "0 2 * * *", "") + &schedule("s2", "0 4 * * *", "")),
        "hi",
    );
    let flow = fx.create(&two).await;
    assert_eq!(
        trigger_ids(&fx.db, flow.id).await,
        vec!["s".to_string(), "s2".to_string()],
        "control: both schedules had a row before the removal"
    );

    fx.update(
        flow.id,
        &flow_yaml("", &schedule("s", "0 2 * * *", ""), "hi"),
    )
    .await;
    assert_eq!(
        trigger_ids(&fx.db, flow.id).await,
        vec!["s".to_string()],
        "the undeclared schedule's row must go, and only it"
    );
}

#[tokio::test]
async fn soft_delete_deletes_every_row() {
    let Some(fx) = fx().await else { return };
    let two = flow_yaml(
        "",
        &(schedule("s", "0 2 * * *", "") + &schedule("s2", "0 4 * * *", "")),
        "hi",
    );
    let flow = fx.create(&two).await;
    assert_eq!(
        trigger_ids(&fx.db, flow.id).await.len(),
        2,
        "control: rows existed before the delete"
    );

    fx.repo.soft_delete(fx.ws, flow.id).await.expect("delete");
    assert_eq!(
        trigger_ids(&fx.db, flow.id).await,
        Vec::<String>::new(),
        "a trashed flow keeps no trigger state (the FK cascade does not fire \
         on a soft delete)"
    );
}

#[tokio::test]
async fn a_disabled_flow_or_trigger_sets_the_flag_and_keeps_the_dates() {
    let Some(fx) = fx().await else { return };

    // Flow-level `disabled: true`.
    let flow = fx.create(CRON_A).await;
    set_sentinel(&fx.db, flow.id, "s").await;
    let off = flow_yaml("disabled: true\n", &schedule("s", "0 2 * * *", ""), "hi");
    assert!(
        parses(&off).disabled,
        "control: the flow-level switch parsed"
    );
    fx.update(flow.id, &off).await;
    assert_eq!(
        row(&fx.db, flow.id, "s").await,
        Some((Some(sentinel()), true, None)),
        "flow-level: disabled set, dates untouched"
    );
    // Re-enabling recovers the same date: nothing was consumed.
    fx.update(
        flow.id,
        &flow_yaml("", &schedule("s", "0 2 * * *", ""), "hi"),
    )
    .await;
    assert_eq!(
        row(&fx.db, flow.id, "s").await,
        Some((Some(sentinel()), false, None)),
        "flow-level: re-enabled, dates still untouched"
    );

    // Trigger-level `disabled: true`, on a second flow.
    let ws2 = common::seed_workspace(&fx.db).await;
    let fx2 = Fx {
        db: fx.db.clone(),
        repo: fx.repo.clone(),
        ws: ws2.0,
        user: ws2.1,
    };
    let flow2 = fx2.create(CRON_A).await;
    set_sentinel(&fx2.db, flow2.id, "s").await;
    let trig_off = flow_yaml(
        "",
        &schedule("s", "0 2 * * *", "    disabled: true\n"),
        "hi",
    );
    assert!(
        parses(&trig_off).triggers[0].disabled && !parses(&trig_off).disabled,
        "control: only the trigger-level switch parsed"
    );
    fx2.update(flow2.id, &trig_off).await;
    assert_eq!(
        row(&fx2.db, flow2.id, "s").await,
        Some((Some(sentinel()), true, None)),
        "trigger-level: disabled set, dates untouched"
    );
}

#[tokio::test]
async fn a_refused_schedule_is_stored_off_queue_with_its_reason() {
    let Some(fx) = fx().await else { return };
    // Six fields with `withSeconds` false: `resolve` refuses it.
    let flow = fx
        .create(&flow_yaml("", &schedule("s", "0 0 * * * *", ""), "hi"))
        .await;
    let (next, disabled, err) = row(&fx.db, flow.id, "s").await.expect("a row, off queue");
    assert_eq!(next, None, "a refused schedule is not queued");
    assert!(!disabled);
    let err = err.expect("the refusal is stored");
    assert!(err.contains("invalid cron"), "{err}");
}

/// Written before the `IS NOT NULL` term existed in the upsert, and watched
/// failing against it (see the task report). `lateMaximumDelay` is refused by
/// `resolve` but is not part of `spec_key`, so without the term the fix would
/// leave the row off the queue for ever.
#[tokio::test]
async fn fixing_a_refused_schedule_puts_it_back_on_the_queue() {
    let Some(fx) = fx().await else { return };
    let refused = flow_yaml(
        "",
        &schedule("s", "0 2 * * *", "    lateMaximumDelay: PT1H\n"),
        "hi",
    );
    let flow = fx.create(&refused).await;
    let (next, _, err) = row(&fx.db, flow.id, "s").await.expect("a row");
    assert_eq!(next, None, "control: refused first");
    assert!(err.expect("reason").contains("lateMaximumDelay"));

    fx.update(
        flow.id,
        &flow_yaml("", &schedule("s", "0 2 * * *", ""), "hi"),
    )
    .await;
    let (next, _, err) = row(&fx.db, flow.id, "s").await.expect("a row");
    let next = next.expect("fixed, so back on the queue");
    assert_eq!((next.hour(), next.minute()), (2, 0));
    assert_eq!(err, None, "the stale refusal is cleared");
}

#[tokio::test]
async fn a_webhook_or_flow_trigger_gets_no_row() {
    let Some(fx) = fx().await else { return };
    let triggers = "  - id: hook\n    type: io.kestra.plugin.core.trigger.Webhook\n    key: k\n  \
                    - id: upstream\n    type: io.kestra.plugin.core.trigger.Flow\n"
        .to_string()
        + &schedule("s", "0 2 * * *", "");
    let src = flow_yaml("", &triggers, "hi");
    let kinds: Vec<_> = parses(&src)
        .triggers
        .iter()
        .map(|t| match t.kind {
            TriggerKind::Schedule(_) => "schedule",
            TriggerKind::Webhook(_) => "webhook",
            TriggerKind::Flow(_) => "flow",
            TriggerKind::Leaf { .. } => "leaf",
        })
        .collect();
    assert_eq!(
        kinds,
        vec!["webhook", "flow", "schedule"],
        "control: the fixture really declares all three kinds"
    );

    let flow = fx.create(&src).await;
    assert_eq!(
        trigger_ids(&fx.db, flow.id).await,
        vec!["s".to_string()],
        "only the schedule gets a row -- and it does get one, so an empty \
         table cannot pass this"
    );
}

/// Every statement that writes `workflow.flows` must sit in a function that
/// calls `scheduler::sync`, or be on [`EXEMPT`] with a reason. The guard
/// against a fifth writer that forgets (spec §3.3).
const EXEMPT: &[(&str, &str)] = &[(
    "set_live",
    "changes neither source nor deleted_at; state is read at fire time",
)];

#[test]
fn every_flows_writer_calls_sync() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    let mut stack = vec![];
    for entry in std::fs::read_dir(&crates).expect("read crates/") {
        let src = entry.expect("entry").path().join("src");
        if src.is_dir() {
            stack.push(src);
        }
    }
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read dir") {
            let p = entry.expect("entry").path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                files.push(p);
            }
        }
    }
    assert!(
        files.len() > 100,
        "scanned only {} files -- is the crates/ walk wrong?",
        files.len()
    );

    // `\` + newline is the string continuation this repo uses inside SQL.
    let stmt = regex::Regex::new(
        r"(?i)(INSERT[\s\\]+INTO|UPDATE|DELETE[\s\\]+FROM)[\s\\]+workflow\.flows\b",
    )
    .unwrap();
    let fn_decl = regex::Regex::new(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap();

    let mut found: Vec<String> = Vec::new();
    let mut offenders: Vec<String> = Vec::new();
    for path in &files {
        let source = std::fs::read_to_string(path).expect("read");
        let shipping = match source.find("#[cfg(test)]") {
            Some(i) => &source[..i],
            None => &source[..],
        };
        // Comments are not statements; a doc line naming a writer must not
        // count as one.
        let code: String = shipping
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        for m in stmt.find_iter(&code) {
            let decl = fn_decl
                .find_iter(&code[..m.start()])
                .last()
                .unwrap_or_else(|| panic!("{}: a writer outside any fn", path.display()));
            let name = fn_decl.captures(decl.as_str()).unwrap()[1].to_string();
            let end = fn_decl
                .find(&code[m.end()..])
                .map_or(code.len(), |n| m.end() + n.start());
            let body = &code[decl.start()..end];
            let who = format!(
                "{}::{name}",
                path.strip_prefix(&crates).unwrap_or(path).display()
            );
            if !body.contains("scheduler::sync(") && !EXEMPT.iter().any(|(n, _)| *n == name) {
                offenders.push(who);
            }
            found.push(name);
        }
    }

    found.sort();
    assert_eq!(
        found,
        vec!["create", "set_live", "soft_delete", "update"],
        "positive control: the scanner must find exactly today's four writers"
    );
    assert!(
        offenders.is_empty(),
        "these write workflow.flows without calling scheduler::sync and are not \
         exempt: {offenders:?}"
    );
}
