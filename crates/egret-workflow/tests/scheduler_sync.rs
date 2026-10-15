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

/// A cron `resolve` ACCEPTS but that has no future instant (`0 0 30 2 *`,
/// 30 February) is off the queue like a refused one -- and must say why, or an
/// author sees a schedule that silently never fires. Final review I4.
#[tokio::test]
async fn a_schedule_that_never_fires_is_stored_off_queue_with_its_reason() {
    let Some(fx) = fx().await else { return };
    let src = flow_yaml("", &schedule("s", "0 0 30 2 *", ""), "hi");
    let t = parses(&src).triggers.remove(0);
    egret_workflow::trigger::ScheduleTrigger::resolve(&t)
        .expect("control: resolve accepts it, so this is not the refusal path");

    let flow = fx.create(&src).await;
    let (next, disabled, err) = row(&fx.db, flow.id, "s").await.expect("a row, off queue");
    assert_eq!(next, None, "no future tick, so nothing to queue");
    assert!(!disabled);
    let err = err.expect("the reason is stored");
    assert!(err.contains("never fires"), "{err}");

    // Control: a fixed cron clears the note and queues the row.
    fx.update(
        flow.id,
        &flow_yaml("", &schedule("s", "0 2 * * *", ""), "hi"),
    )
    .await;
    let (next, _, err) = row(&fx.db, flow.id, "s").await.expect("a row");
    assert!(next.is_some(), "back on the queue");
    assert_eq!(err, None);
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
///
/// **What the scanner cannot see** (final review M5). It reads Rust source
/// text under `crates/*/src` with one regex, so a writer is invisible to it
/// when:
///
/// - it is SQL outside Rust: a data migration under `migrations/`, a
///   function or trigger defined in SQL, a script;
/// - the statement text is split so the regex cannot match it, e.g. by
///   `concat!("UPDATE workflow.", "flows ...")` or `format!` pieces;
/// - the table name is built at run time (`format!("{schema}.flows")`, a
///   bound identifier, a `const` holding the name);
/// - it sits inside a `/* ... */` block comment -- only `//` lines are
///   stripped, so such text counts as code (a false POSITIVE, not a miss);
/// - it comes after the file's first `#[cfg(test)]`, which is taken as the
///   start of the test half even when shipping code follows it.
///
/// It also judges by function body text: a writer whose `sync` call lives in
/// a helper it calls, or whose body carries `scheduler::sync(` only in a
/// trailing `//` comment or a string, is judged by that text, not by what
/// runs.
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

// ── W7e Task 2: the runtime switch (spec 2026-10-14 §3.2, §3.4) ─────────────
//
// The switch is a row in `workflow.disabled_triggers`, staged here by SQL
// (`common::switch_off`) because its route arrives in a later task. The flow
// is still written only through `FlowRepo`: what is under test is that a save
// copies the switch into `workflow.triggers.disabled` and keeps the switch
// rows in step with the declared ids.

const WEBHOOK: &str = "  - id: w\n    type: io.kestra.plugin.core.trigger.Webhook\n    key: k\n";

/// Every cause of `disabled`, each ALONE, writes `true`; with none of them the
/// same save writes `false`. The runtime cause also keeps the dates: a switch
/// never touches `next_evaluation_date`.
#[tokio::test]
async fn each_cause_alone_disables_the_schedule_row() {
    // Flow-level YAML.
    let Some(by_flow) = fx().await else { return };
    let flow = by_flow
        .create(&flow_yaml(
            "disabled: true\n",
            &schedule("s", "0 2 * * *", ""),
            "hi",
        ))
        .await;
    assert_eq!(
        row(&by_flow.db, flow.id, "s").await.map(|r| r.1),
        Some(true),
        "flow YAML"
    );

    // Trigger-level YAML.
    let Some(by_trigger) = fx().await else { return };
    let flow = by_trigger
        .create(&flow_yaml(
            "",
            &schedule("s", "0 2 * * *", "    disabled: true\n"),
            "hi",
        ))
        .await;
    assert_eq!(
        row(&by_trigger.db, flow.id, "s").await.map(|r| r.1),
        Some(true),
        "trigger YAML"
    );

    // The runtime switch, then a save that keeps the cron.
    let Some(fx) = fx().await else { return };
    let flow = fx.create(CRON_A).await;
    assert_eq!(
        row(&fx.db, flow.id, "s").await.map(|r| r.1),
        Some(false),
        "control: no cause, not disabled"
    );
    set_sentinel(&fx.db, flow.id, "s").await;
    common::switch_off(&fx.db, flow.id, "s").await;
    let edited = CRON_A.replace("message: hi", "message: saved");
    fx.update(flow.id, &edited).await;
    assert_eq!(
        row(&fx.db, flow.id, "s").await,
        Some((Some(sentinel()), true, None)),
        "the runtime switch: disabled, dates untouched"
    );

    // Control: the same save with the switch row gone writes `false` again,
    // so the `true` above came from the row and not from the save.
    common::switch_on(&fx.db, flow.id, "s").await;
    fx.update(flow.id, CRON_A).await;
    assert_eq!(
        row(&fx.db, flow.id, "s").await,
        Some((Some(sentinel()), false, None)),
        "switched back on: enabled, dates still untouched"
    );
}

/// §3.4: a save deletes the switch rows of ids the source no longer declares,
/// under any kind, and keeps the rest; a renamed id starts on; a soft delete
/// deletes them all; another flow's rows are never touched.
#[tokio::test]
async fn a_save_keeps_the_switch_rows_of_declared_ids_only() {
    let Some(fx) = fx().await else { return };
    let ab = |a: &str| {
        flow_yaml(
            "",
            &(schedule(a, "0 2 * * *", "") + &schedule("b", "0 4 * * *", "") + WEBHOOK),
            "hi",
        )
    };
    let flow = fx.create(&ab("a")).await;
    // Another flow of the same workspace, switched off under the same ids.
    let other = common::draft(
        &fx.repo,
        fx.ws,
        fx.user,
        &ab("a").replacen("id: f\n", "id: g\n", 1),
    )
    .await;
    for t in ["a", "b", "w"] {
        common::switch_off(&fx.db, flow.id, t).await;
        common::switch_off(&fx.db, other.id, t).await;
    }
    let all = vec!["a".to_string(), "b".into(), "w".into()];

    // Dropping `b`.
    let no_b = flow_yaml("", &(schedule("a", "0 2 * * *", "") + WEBHOOK), "hi");
    fx.update(flow.id, &no_b).await;
    assert_eq!(
        common::switched_off(&fx.db, flow.id).await,
        vec!["a".to_string(), "w".into()],
        "`b`'s row goes; `a`'s stays, and so does the webhook's although it has \
         no workflow.triggers row"
    );
    assert_eq!(
        row(&fx.db, flow.id, "a").await.map(|r| r.1),
        Some(true),
        "`a` still off"
    );

    // Renaming `a` to `a2`: a new trigger, so it starts on.
    let renamed = flow_yaml("", &(schedule("a2", "0 2 * * *", "") + WEBHOOK), "hi");
    fx.update(flow.id, &renamed).await;
    assert_eq!(
        common::switched_off(&fx.db, flow.id).await,
        vec!["w".to_string()],
        "the old id's row goes with it"
    );
    assert_eq!(
        row(&fx.db, flow.id, "a2").await.map(|r| r.1),
        Some(false),
        "`a2` starts on"
    );
    assert_eq!(
        common::switched_off(&fx.db, other.id).await,
        all,
        "other flow untouched"
    );

    // Soft delete.
    fx.repo.soft_delete(fx.ws, flow.id).await.expect("delete");
    assert_eq!(
        common::switched_off(&fx.db, flow.id).await,
        Vec::<String>::new(),
        "a trashed flow keeps no switch (the FK cascade does not fire on a soft delete)"
    );
    assert_eq!(
        common::switched_off(&fx.db, other.id).await,
        all,
        "other flow untouched"
    );
}

/// §3.4: the switch is per id, whatever the kind -- a `Schedule` `t` that
/// becomes a `Webhook` `t` keeps its row.
#[tokio::test]
async fn a_kind_change_under_the_same_id_keeps_the_switch() {
    let Some(fx) = fx().await else { return };
    let flow = fx
        .create(&flow_yaml("", &schedule("t", "0 2 * * *", ""), "hi"))
        .await;
    common::switch_off(&fx.db, flow.id, "t").await;

    let hook = flow_yaml(
        "",
        "  - id: t\n    type: io.kestra.plugin.core.trigger.Webhook\n    key: k\n",
        "hi",
    );
    assert!(
        matches!(parses(&hook).triggers[0].kind, TriggerKind::Webhook(_)),
        "control: `t` is a webhook now"
    );
    fx.update(flow.id, &hook).await;
    assert_eq!(
        trigger_ids(&fx.db, flow.id).await,
        Vec::<String>::new(),
        "control: the schedule row went with the kind"
    );
    assert_eq!(
        common::switched_off(&fx.db, flow.id).await,
        vec!["t".to_string()]
    );
}

// ---------------------------------------------------------------------------
// W7f Task 5 (spec §3.7, D19, D20): `sync` re-derives a schedule from its
// backfill and owns the backfill rows' lifecycle. Backfills are staged with
// `common::stage_backfill` (the create route is Task 7's); the dates a fire
// would have advanced are moved by SQL on both rows, as a fire writes them.
// ---------------------------------------------------------------------------

/// The flow under test: one schedule `s` on `cron`, its task saying `message`
/// (a different message is "a task edit").
fn one_schedule(cron: &str, message: &str) -> String {
    flow_yaml("", &schedule("s", cron, ""), message)
}

/// The top of the DB clock's hour, two hours back: a past, even-minute
/// origin for a window of one-minute ticks.
async fn origin(db: &Db) -> DateTime<Utc> {
    let now = db_now(db).await;
    now.date_naive()
        .and_time(NaiveTime::from_hms_opt(now.hour(), 0, 0).unwrap())
        .and_utc()
        - Duration::hours(2)
}

/// Stage a backfill of `s` over `[w0, w0 + 9min]` (ten ticks of `* * * * *`),
/// as if `done` ticks had fired, and pin the regular schedule's stored date
/// to `prev` (both rows, as a fire and the create route write them).
async fn stage_minutes(
    db: &Db,
    flow: Uuid,
    trig: &str,
    w0: DateTime<Utc>,
    done: i32,
    prev: DateTime<Utc>,
) {
    common::stage_backfill(
        db,
        flow,
        trig,
        (w0, w0 + Duration::minutes(9)),
        w0,
        10,
        serde_json::json!({}),
        serde_json::json!([]),
    )
    .await;
    move_backfill(db, flow, trig, w0 + Duration::minutes(done.into()), done).await;
    sqlx::query(
        "UPDATE workflow.trigger_backfills SET previous_next_evaluation_date = $3 \
          WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(flow)
    .bind(trig)
    .bind(prev)
    .execute(&db.pool)
    .await
    .unwrap();
}

/// What a fire writes when it advances a backfill: `current_tick` and
/// `ticks_done` on the backfill row, the same date on the trigger row.
async fn move_backfill(db: &Db, flow: Uuid, trig: &str, tick: DateTime<Utc>, done: i32) {
    let n = sqlx::query(
        "UPDATE workflow.trigger_backfills SET current_tick = $3, ticks_done = $4 \
          WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(flow)
    .bind(trig)
    .bind(tick)
    .bind(done)
    .execute(&db.pool)
    .await
    .unwrap()
    .rows_affected();
    assert_eq!(n, 1, "the backfill row to move");
    sqlx::query(
        "UPDATE workflow.triggers SET next_evaluation_date = $3 \
          WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(flow)
    .bind(trig)
    .bind(tick)
    .execute(&db.pool)
    .await
    .unwrap();
}

/// `(current_tick, total_ticks, ticks_done, previous_next_evaluation_date)`.
type BackfillDates = (DateTime<Utc>, i32, i32, DateTime<Utc>);

async fn backfill(db: &Db, flow: Uuid, trig: &str) -> Option<BackfillDates> {
    sqlx::query_as(
        "SELECT current_tick, total_ticks, ticks_done, previous_next_evaluation_date \
           FROM workflow.trigger_backfills WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(flow)
    .bind(trig)
    .fetch_optional(&db.pool)
    .await
    .unwrap()
}

/// The trigger row's queue state, whole: `(next_evaluation_date,
/// evaluated_at, last_error)`.
type QueueState = (Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<String>);

async fn queue(db: &Db, flow: Uuid, trig: &str) -> Option<QueueState> {
    sqlx::query_as(
        "SELECT next_evaluation_date, evaluated_at, last_error FROM workflow.triggers \
          WHERE flow_id = $1 AND trigger_id = $2",
    )
    .bind(flow)
    .bind(trig)
    .fetch_optional(&db.pool)
    .await
    .unwrap()
}

/// The first even minute at or after `t` (a whole minute): `*/2 * * * *`.
fn even_at_or_after(t: DateTime<Utc>) -> DateTime<Utc> {
    if t.minute().is_multiple_of(2) {
        t
    } else {
        t + Duration::minutes(1)
    }
}

const P2: &str = "the backfill ended: the new schedule gives more than 10000 ticks in its window";

impl Fx {
    /// `create`, under another `flow_id` (a second flow of the workspace).
    async fn create_as(&self, flow_id: &str, src: &str) -> FlowRow {
        parses(src);
        self.repo
            .create(
                self.ws,
                self.user,
                &CreateFlowInput {
                    namespace: "t".into(),
                    flow_id: flow_id.into(),
                    source: src.into(),
                    plan: serde_json::json!({}),
                    folder_id: None,
                },
            )
            .await
            .expect("create")
    }
}

#[tokio::test]
async fn a_save_that_keeps_the_cron_leaves_the_backfill_and_the_dates() {
    let Some(fx) = fx().await else { return };
    let flow = fx.create(&one_schedule("* * * * *", "hi")).await;
    let w0 = origin(&fx.db).await;
    stage_minutes(&fx.db, flow.id, "s", w0, 3, w0 + Duration::hours(3)).await;
    let row_before = common::backfill_row(&fx.db, flow.id, "s").await;
    let queue_before = queue(&fx.db, flow.id, "s").await;
    assert_eq!(
        queue_before.as_ref().map(|q| q.0),
        Some(Some(w0 + Duration::minutes(3))),
        "control: the trigger follows the backfill"
    );

    fx.update(flow.id, &one_schedule("* * * * *", "edited"))
        .await;

    assert_eq!(common::backfill_row(&fx.db, flow.id, "s").await, row_before);
    assert_eq!(queue(&fx.db, flow.id, "s").await, queue_before);
}

#[tokio::test]
async fn a_cron_change_mid_backfill_re_derives_from_the_current_tick() {
    let Some(fx) = fx().await else { return };
    // `current_tick` on an odd minute: it moves to the next even one.
    let flow = fx.create(&one_schedule("* * * * *", "hi")).await;
    let w0 = origin(&fx.db).await;
    let prev = w0 + Duration::hours(3) + Duration::minutes(1);
    stage_minutes(&fx.db, flow.id, "s", w0, 3, prev).await;
    sqlx::query(
        "UPDATE workflow.triggers SET evaluated_at = $2, last_error = 'x' WHERE flow_id = $1",
    )
    .bind(flow.id)
    .bind(w0)
    .execute(&fx.db.pool)
    .await
    .unwrap();

    fx.update(flow.id, &one_schedule("*/2 * * * *", "hi")).await;

    let cur = w0 + Duration::minutes(4);
    assert_eq!(cur, even_at_or_after(w0 + Duration::minutes(3)));
    // Left under `*/2` in [w0+4, w0+9]: w0+4, w0+6, w0+8.
    assert_eq!(
        backfill(&fx.db, flow.id, "s").await,
        Some((cur, 3 + 3, 3, even_at_or_after(prev))),
        "current_tick re-derived, total recounted, prev re-snapped, done kept"
    );
    assert_eq!(
        queue(&fx.db, flow.id, "s").await,
        Some((Some(cur), None, None)),
        "the trigger follows the backfill, re-initialised"
    );

    // On an even minute, the current tick is kept exactly.
    let flow = fx.create_as("g", &one_schedule("* * * * *", "hi")).await;
    stage_minutes(&fx.db, flow.id, "s", w0, 4, prev).await;
    fx.update(flow.id, &one_schedule("*/2 * * * *", "hi")).await;
    assert_eq!(
        backfill(&fx.db, flow.id, "s").await,
        Some((w0 + Duration::minutes(4), 4 + 3, 4, even_at_or_after(prev))),
    );
    assert_eq!(
        queue(&fx.db, flow.id, "s").await.map(|q| q.0),
        Some(Some(w0 + Duration::minutes(4)))
    );
}

#[tokio::test]
async fn a_cron_change_past_the_window_completes_the_backfill() {
    let Some(fx) = fx().await else { return };
    let flow = fx.create(&one_schedule("* * * * *", "hi")).await;
    let w0 = origin(&fx.db).await;
    // An odd minute, in the future: the resume keeps it, snapped to `*/2`.
    let prev = w0 + Duration::hours(3) + Duration::minutes(1);
    // The last tick, w0+9, odd: `*/2`'s next is w0+10, past `window_end`.
    stage_minutes(&fx.db, flow.id, "s", w0, 9, prev).await;

    fx.update(flow.id, &one_schedule("*/2 * * * *", "hi")).await;

    assert_eq!(
        backfill(&fx.db, flow.id, "s").await,
        None,
        "complete: the row goes"
    );
    assert_eq!(
        queue(&fx.db, flow.id, "s").await,
        Some((Some(even_at_or_after(prev)), None, None)),
        "the regular schedule resumes at its stored date, on the new cron"
    );
}

#[tokio::test]
async fn a_cron_change_over_the_cap_ends_the_backfill_with_a_reason() {
    let Some(fx) = fx().await else { return };
    let flow = fx.create(&one_schedule("0 0 * * *", "hi")).await;
    let now = db_now(&fx.db).await;
    let midnight = now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
    let d0 = midnight - Duration::days(9);
    // Eight daily ticks, none done; under `* * * * *` the window holds
    // 7 * 1440 + 1 = 10,081.
    common::stage_backfill(
        &fx.db,
        flow.id,
        "s",
        (d0, d0 + Duration::days(7)),
        d0,
        8,
        serde_json::json!({}),
        serde_json::json!([]),
    )
    .await;
    let prev = midnight + Duration::days(1);
    sqlx::query("UPDATE workflow.trigger_backfills SET previous_next_evaluation_date = $2 WHERE flow_id = $1")
        .bind(flow.id)
        .bind(prev)
        .execute(&fx.db.pool)
        .await
        .unwrap();

    fx.update(flow.id, &one_schedule("* * * * *", "hi")).await;

    assert_eq!(
        backfill(&fx.db, flow.id, "s").await,
        None,
        "ended: the row goes"
    );
    assert_eq!(
        queue(&fx.db, flow.id, "s").await,
        Some((Some(prev), None, Some(P2.to_string()))),
        "resumed as a delete does, and says why"
    );
}

#[tokio::test]
async fn a_cron_with_no_tick_ends_the_backfill_off_the_queue() {
    let Some(fx) = fx().await else { return };
    let flow = fx.create(&one_schedule("* * * * *", "hi")).await;
    let w0 = origin(&fx.db).await;
    let prev = w0 + Duration::hours(3) + Duration::minutes(1);
    stage_minutes(&fx.db, flow.id, "s", w0, 3, prev).await;

    fx.update(flow.id, &one_schedule("0 0 30 2 *", "hi")).await;

    // No tick at or after `current_tick`: complete. No tick at or after the
    // stored `prev` either (D20): the stored date is kept for the resume,
    // which finds no tick of the new cron there, so the row goes off the
    // queue with `never_fires`.
    assert_eq!(backfill(&fx.db, flow.id, "s").await, None);
    let (next, evaluated, err) = queue(&fx.db, flow.id, "s").await.expect("the row");
    assert_eq!((next, evaluated), (None, None));
    let err = err.expect("a reason");
    assert!(
        err.starts_with("the schedule never fires: `0 0 30 2 *` has no tick after"),
        "{err}"
    );
}

#[tokio::test]
async fn a_trigger_off_the_queue_is_re_queued_at_the_current_tick() {
    let Some(fx) = fx().await else { return };
    let flow = fx.create(&one_schedule("* * * * *", "hi")).await;
    let w0 = origin(&fx.db).await;
    stage_minutes(&fx.db, flow.id, "s", w0, 3, w0 + Duration::hours(3)).await;
    // As `fire::take_off_queue` leaves it.
    sqlx::query(
        "UPDATE workflow.triggers SET next_evaluation_date = NULL, last_error = 'refused' \
          WHERE flow_id = $1",
    )
    .bind(flow.id)
    .execute(&fx.db.pool)
    .await
    .unwrap();

    fx.update(flow.id, &one_schedule("* * * * *", "edited"))
        .await;

    let cur = w0 + Duration::minutes(3);
    assert_eq!(
        queue(&fx.db, flow.id, "s").await,
        Some((Some(cur), None, None)),
        "re-queued at the backfill's tick, not at the next tick after now"
    );
    assert_eq!(
        backfill(&fx.db, flow.id, "s").await,
        Some((cur, 10, 3, w0 + Duration::hours(3)))
    );
}

#[tokio::test]
async fn an_unparsable_source_keeps_the_backfill_and_a_fix_re_derives() {
    let Some(fx) = fx().await else { return };
    let flow = fx.create(&one_schedule("* * * * *", "hi")).await;
    let w0 = origin(&fx.db).await;
    stage_minutes(&fx.db, flow.id, "s", w0, 3, w0 + Duration::hours(3)).await;
    let staged = common::backfill_row(&fx.db, flow.id, "s").await;
    sqlx::query("UPDATE workflow.flows SET source = 'not: [valid' WHERE id = $1")
        .bind(flow.id)
        .execute(&fx.db.pool)
        .await
        .unwrap();

    egret_workflow::scheduler::reconcile(&fx.db.pool)
        .await
        .expect("reconcile");
    assert_eq!(
        trigger_ids(&fx.db, flow.id).await,
        Vec::<String>::new(),
        "control: the trigger row went with the source"
    );
    assert_eq!(
        common::backfill_row(&fx.db, flow.id, "s").await,
        staged,
        "the backfill is kept, untouched"
    );

    fx.update(flow.id, &one_schedule("* * * * *", "fixed"))
        .await;
    assert_eq!(
        queue(&fx.db, flow.id, "s").await,
        Some((Some(w0 + Duration::minutes(3)), None, None)),
        "re-created at the backfill's tick"
    );
}

#[tokio::test]
async fn removing_a_kind_change_and_a_soft_delete_delete_the_backfills() {
    let Some(fx) = fx().await else { return };
    let three = flow_yaml(
        "",
        &(schedule("s", "* * * * *", "")
            + &schedule("s2", "* * * * *", "")
            + &schedule("s3", "* * * * *", "")),
        "hi",
    );
    let flow = fx.create(&three).await;
    let other = fx.create_as("g", &one_schedule("* * * * *", "hi")).await;
    let w0 = origin(&fx.db).await;
    let prev = w0 + Duration::hours(3);
    for t in ["s", "s2", "s3"] {
        stage_minutes(&fx.db, flow.id, t, w0, 3, prev).await;
    }
    stage_minutes(&fx.db, other.id, "s", w0, 3, prev).await;
    let other_before = common::backfill_row(&fx.db, other.id, "s").await;
    let other_queue = queue(&fx.db, other.id, "s").await;
    let kept = |db: &Db| {
        let db = db.clone();
        let other = other.id;
        let (row, q) = (other_before.clone(), other_queue.clone());
        async move {
            assert_eq!(
                common::backfill_row(&db, other, "s").await,
                row,
                "another flow's backfill"
            );
            assert_eq!(queue(&db, other, "s").await, q, "another flow's trigger");
        }
    };

    // A removed id: its backfill goes, the others stay.
    let s_queue = queue(&fx.db, flow.id, "s").await;
    fx.update(
        flow.id,
        &flow_yaml(
            "",
            &(schedule("s", "* * * * *", "") + &schedule("s3", "* * * * *", "")),
            "hi",
        ),
    )
    .await;
    assert_eq!(backfill(&fx.db, flow.id, "s2").await, None, "removed id");
    assert!(backfill(&fx.db, flow.id, "s").await.is_some());
    assert!(backfill(&fx.db, flow.id, "s3").await.is_some());
    assert_eq!(
        queue(&fx.db, flow.id, "s").await,
        s_queue,
        "no resume elsewhere"
    );
    kept(&fx.db).await;

    // A kind change under the same id.
    let hook = flow_yaml(
        "",
        &("  - id: s\n    type: io.kestra.plugin.core.trigger.Webhook\n    key: k\n".to_string()
            + &schedule("s3", "* * * * *", "")),
        "hi",
    );
    fx.update(flow.id, &hook).await;
    assert_eq!(backfill(&fx.db, flow.id, "s").await, None, "kind change");
    assert!(backfill(&fx.db, flow.id, "s3").await.is_some());
    kept(&fx.db).await;

    // A soft delete: every backfill of the flow.
    fx.repo.soft_delete(fx.ws, flow.id).await.expect("delete");
    assert_eq!(backfill(&fx.db, flow.id, "s3").await, None, "soft delete");
    kept(&fx.db).await;
}

#[tokio::test]
async fn reconcile_over_an_unchanged_backfill_changes_nothing() {
    let Some(fx) = fx().await else { return };
    let flow = fx.create(&one_schedule("* * * * *", "hi")).await;
    let w0 = origin(&fx.db).await;
    stage_minutes(&fx.db, flow.id, "s", w0, 3, w0 + Duration::hours(3)).await;
    let row_before = common::backfill_row(&fx.db, flow.id, "s").await;
    let queue_before = queue(&fx.db, flow.id, "s").await;

    egret_workflow::scheduler::reconcile(&fx.db.pool)
        .await
        .expect("reconcile");

    assert_eq!(common::backfill_row(&fx.db, flow.id, "s").await, row_before);
    assert_eq!(queue(&fx.db, flow.id, "s").await, queue_before);
}

/// D20 on the branch where the backfill goes on: the new cron has ticks in
/// the window but none at or after the stored `previous_next_evaluation_date`
/// (a date in chrono's last year stands in, since every cron that ticks once
/// ticks again within the search bound). The stored date is kept; the save
/// succeeds.
#[tokio::test]
async fn a_re_snap_with_no_tick_keeps_the_stored_date() {
    let Some(fx) = fx().await else { return };
    let flow = fx.create(&one_schedule("0 0 1 1 *", "hi")).await;
    let y = |y: i32, m: u32| Utc.with_ymd_and_hms(y, m, 1, 0, 0, 0).unwrap();
    common::stage_backfill(
        &fx.db,
        flow.id,
        "s",
        (y(2020, 1), y(2022, 1)),
        y(2020, 1),
        3,
        serde_json::json!({}),
        serde_json::json!([]),
    )
    .await;
    let prev = chrono::NaiveDate::MAX
        .pred_opt()
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    sqlx::query("UPDATE workflow.trigger_backfills SET previous_next_evaluation_date = $2 WHERE flow_id = $1")
        .bind(flow.id)
        .bind(prev)
        .execute(&fx.db.pool)
        .await
        .unwrap();

    fx.update(flow.id, &one_schedule("0 0 1 1,7 *", "hi")).await;

    // Under the new cron: 2020-01, 2020-07, 2021-01, 2021-07, 2022-01.
    assert_eq!(
        backfill(&fx.db, flow.id, "s").await,
        Some((y(2020, 1), 5, 0, prev))
    );
    assert_eq!(
        queue(&fx.db, flow.id, "s").await.map(|q| q.0),
        Some(Some(y(2020, 1)))
    );
}
