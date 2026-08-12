//! DB-gated `ScheduleRepo` tests. Self-skip without DATABASE_URL, following
//! `egret-bi/tests/dashboard_repo.rs`.

use std::collections::HashMap;

use egret_automation::{NewSchedule, ScheduleRepo};
use egret_metastore::Db;
use uuid::Uuid;

struct Fixture {
    repo: ScheduleRepo,
    ws: Uuid,
    /// A second workspace, used by every cross-tenant assertion.
    other_ws: Uuid,
    user: Uuid,
    other_user: Uuid,
    database: Uuid,
    db: Db,
}

async fn setup() -> Option<Fixture> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let mk_user = |db: Db| async move {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id",
        )
        .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .unwrap()
    };
    let user = mk_user(db.clone()).await;
    let other_user = mk_user(db.clone()).await;

    let mk_ws = |db: Db, owner: Uuid| async move {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t',$2) RETURNING id",
        )
        .bind(format!("w-{}", Uuid::new_v4().simple()))
        .bind(owner)
        .fetch_one(&db.pool)
        .await
        .unwrap()
    };
    let ws = mk_ws(db.clone(), user).await;
    let other_ws = mk_ws(db.clone(), user).await;

    let database: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id) \
         VALUES ($1,'trino','localhost',8080,$2) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    Some(Fixture {
        repo: ScheduleRepo::new(db.clone()),
        ws,
        other_ws,
        user,
        other_user,
        database,
        db,
    })
}

/// A valid alert, the minimum that satisfies `schedule_alert_needs_query`.
fn alert(f: &Fixture, name: &str) -> NewSchedule {
    NewSchedule {
        r#type: "alert".into(),
        name: name.into(),
        description: String::new(),
        active: true,
        crontab: "0 9 * * *".into(),
        timezone: "UTC".into(),
        database_id: Some(f.database),
        sql: Some("SELECT count(*) FROM orders".into()),
        validator_type: Some("not_null".into()),
        validator_config: serde_json::json!({}),
        chart_id: None,
        dashboard_id: None,
        report_format: None,
        custom_width: None,
        custom_height: None,
        grace_period_s: 14400,
        working_timeout_s: 3600,
        log_retention_days: 90,
        email_subject: None,
        owners: vec![f.user],
        recipients: vec![(
            "email".into(),
            serde_json::json!({"addresses": ["on-call@t.local"]}),
        )],
    }
}

macro_rules! fixture {
    () => {
        match setup().await {
            Some(f) => f,
            None => {
                eprintln!("skip: DATABASE_URL unset");
                return;
            }
        }
    };
}

#[tokio::test]
async fn create_then_get_round_trips_every_field() {
    let f = fixture!();
    let mut input = alert(&f, "orders dropped");
    input.description = "watch order volume".into();
    input.email_subject = Some("[alert] orders".into());
    input.grace_period_s = 600;

    let made = f
        .repo
        .create(f.ws, &input, Some(egret_core::UserId(f.user)))
        .await
        .unwrap();
    let got = f.repo.get(f.ws, made.id).await.unwrap().expect("present");

    assert_eq!(got.name, "orders dropped");
    assert_eq!(got.description, "watch order volume");
    assert_eq!(got.r#type, "alert");
    assert_eq!(got.sql.as_deref(), Some("SELECT count(*) FROM orders"));
    assert_eq!(got.validator_type.as_deref(), Some("not_null"));
    assert_eq!(got.email_subject.as_deref(), Some("[alert] orders"));
    assert_eq!(got.grace_period_s, 600);
    assert_eq!(got.created_by, Some(f.user));
    assert!(got.active);
}

#[tokio::test]
async fn create_computes_next_run_at_from_the_crontab() {
    // A row with a NULL next_run_at is one the scheduler's claim query
    // (`WHERE active AND next_run_at <= now()`) will never match — the schedule
    // would look created and simply never fire.
    let f = fixture!();
    let made = f
        .repo
        .create(
            f.ws,
            &alert(&f, "has next run"),
            Some(egret_core::UserId(f.user)),
        )
        .await
        .unwrap();

    let next = made
        .next_run_at
        .expect("next_run_at must be set at write time");
    assert!(
        next > chrono::Utc::now(),
        "next run must be in the future, got {next}"
    );
}

#[tokio::test]
async fn list_returns_only_the_given_workspaces_schedules() {
    let f = fixture!();
    f.repo.create(f.ws, &alert(&f, "mine"), None).await.unwrap();
    // Same name in the other workspace: the unique constraint is per-workspace,
    // so this must be allowed AND must not leak into the first list.
    let mut theirs = alert(&f, "mine");
    theirs.database_id = None;
    theirs.sql = Some("SELECT 1".into());
    theirs.database_id = Some(other_ws_database(&f).await);
    f.repo.create(f.other_ws, &theirs, None).await.unwrap();

    let mine = f.repo.list(f.ws, None).await.unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].workspace_id, f.ws);
}

#[tokio::test]
async fn list_filters_by_type_for_the_two_tabs() {
    let f = fixture!();
    f.repo
        .create(f.ws, &alert(&f, "an alert"), None)
        .await
        .unwrap();

    let alerts = f.repo.list(f.ws, Some("alert")).await.unwrap();
    assert_eq!(alerts.len(), 1);
    let reports = f.repo.list(f.ws, Some("report")).await.unwrap();
    assert!(reports.is_empty(), "the Reports tab must not show alerts");
}

#[tokio::test]
async fn another_workspaces_schedule_is_invisible_not_merely_unwritable() {
    // The IDOR rule: reads are filtered by workspace too, so the route layer
    // can return 404 rather than 403 and not disclose existence.
    let f = fixture!();
    let made = f
        .repo
        .create(f.ws, &alert(&f, "secret"), None)
        .await
        .unwrap();

    assert!(f.repo.get(f.other_ws, made.id).await.unwrap().is_none());
    assert!(f
        .repo
        .update(f.other_ws, made.id, &alert(&f, "renamed"), None)
        .await
        .unwrap()
        .is_none());
    assert!(!f.repo.delete(f.other_ws, made.id).await.unwrap());

    // ...and the row is untouched by any of it.
    let still = f
        .repo
        .get(f.ws, made.id)
        .await
        .unwrap()
        .expect("still there");
    assert_eq!(still.name, "secret");
}

#[tokio::test]
async fn a_duplicate_name_within_one_type_is_a_conflict() {
    let f = fixture!();
    f.repo.create(f.ws, &alert(&f, "dupe"), None).await.unwrap();

    let err = f
        .repo
        .create(f.ws, &alert(&f, "dupe"), None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, egret_automation::AutomationError::Conflict(_)),
        "expected Conflict, got {err:?}"
    );
}

#[tokio::test]
async fn the_same_name_may_be_used_by_an_alert_and_a_report() {
    // The unique key is (workspace, TYPE, name): the two tabs are separate
    // namespaces, so "daily numbers" can be both an alert and a report.
    let f = fixture!();
    f.repo
        .create(f.ws, &alert(&f, "daily numbers"), None)
        .await
        .unwrap();

    let mut report = alert(&f, "daily numbers");
    report.r#type = "report".into();
    report.database_id = None;
    report.sql = None;
    report.validator_type = None;
    report.report_format = Some("png".into());
    report.dashboard_id = Some(a_dashboard(&f).await);

    let made = f.repo.create(f.ws, &report, None).await.unwrap();
    assert_eq!(made.r#type, "report");
    assert_eq!(made.report_format.as_deref(), Some("png"));
}

#[tokio::test]
async fn update_replaces_owners_and_recipients_rather_than_appending() {
    // The editor submits the full list, so a save that appended would silently
    // keep mailing a removed recipient — the worst kind of stale state.
    let f = fixture!();
    let made = f.repo.create(f.ws, &alert(&f, "swap"), None).await.unwrap();

    let mut changed = alert(&f, "swap");
    changed.owners = vec![f.other_user];
    changed.recipients = vec![(
        "webhook".into(),
        serde_json::json!({"url": "https://hooks.t.local/x"}),
    )];
    f.repo
        .update(f.ws, made.id, &changed, None)
        .await
        .unwrap()
        .expect("updated");

    let owners = f.repo.owners_for(&[made.id]).await.unwrap();
    let ids: Vec<Uuid> = owners[&made.id].iter().map(|o| o.user_id).collect();
    assert_eq!(ids, vec![f.other_user], "old owner must be gone");

    let recips = f.repo.recipients_for(made.id).await.unwrap();
    assert_eq!(recips.len(), 1, "old recipient must be gone");
    assert_eq!(recips[0].kind, "webhook");
}

#[tokio::test]
async fn update_recomputes_next_run_at_when_the_crontab_changes() {
    let f = fixture!();
    let made = f
        .repo
        .create(f.ws, &alert(&f, "retimed"), None)
        .await
        .unwrap();
    let before = made.next_run_at.expect("set on create");

    let mut changed = alert(&f, "retimed");
    // Every minute — the next run must move much closer than tomorrow 09:00.
    changed.crontab = "* * * * *".into();
    let after = f
        .repo
        .update(f.ws, made.id, &changed, None)
        .await
        .unwrap()
        .expect("updated")
        .next_run_at
        .expect("still set");

    assert!(
        after < before,
        "a more frequent crontab must move next_run_at earlier: {after} vs {before}"
    );
}

#[tokio::test]
async fn owners_for_batches_several_schedules_in_one_call() {
    // The list page renders an owner column; a query per row is what this
    // signature exists to prevent.
    let f = fixture!();
    let a = f.repo.create(f.ws, &alert(&f, "one"), None).await.unwrap();
    let mut second = alert(&f, "two");
    second.owners = vec![f.user, f.other_user];
    let b = f.repo.create(f.ws, &second, None).await.unwrap();

    let map: HashMap<Uuid, Vec<_>> = f.repo.owners_for(&[a.id, b.id]).await.unwrap();
    assert_eq!(map[&a.id].len(), 1);
    assert_eq!(map[&b.id].len(), 2);
    assert!(
        map[&a.id][0].email.contains('@'),
        "email is joined for display"
    );
}

#[tokio::test]
async fn deleting_a_schedule_takes_its_owners_recipients_and_runs_with_it() {
    let f = fixture!();
    let made = f
        .repo
        .create(f.ws, &alert(&f, "doomed"), None)
        .await
        .unwrap();
    sqlx::query("INSERT INTO automation.schedule_runs (schedule_id, state) VALUES ($1,'success')")
        .bind(made.id)
        .execute(&f.db.pool)
        .await
        .unwrap();

    assert!(f.repo.delete(f.ws, made.id).await.unwrap());

    for table in [
        "automation.schedule_owners",
        "automation.schedule_recipients",
        "automation.schedule_runs",
    ] {
        let left: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM {table} WHERE schedule_id = $1"
        ))
        .bind(made.id)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
        assert_eq!(left, 0, "{table} still has rows for a deleted schedule");
    }
}

#[tokio::test]
async fn runs_come_back_newest_first_and_only_for_this_workspace() {
    let f = fixture!();
    let made = f
        .repo
        .create(f.ws, &alert(&f, "logged"), None)
        .await
        .unwrap();
    for (state, ago) in [("error", 60), ("success", 30), ("noop", 1)] {
        sqlx::query(
            "INSERT INTO automation.schedule_runs (schedule_id, state, started_at) \
             VALUES ($1,$2, now() - ($3 || ' minutes')::interval)",
        )
        .bind(made.id)
        .bind(state)
        .bind(ago.to_string())
        .execute(&f.db.pool)
        .await
        .unwrap();
    }

    let runs = f.repo.runs(f.ws, made.id, 10).await.unwrap();
    let states: Vec<&str> = runs.iter().map(|r| r.state.as_str()).collect();
    assert_eq!(states, vec!["noop", "success", "error"], "newest first");

    // The run log is reached through the schedule, so it must be workspace
    // scoped too — otherwise the 404 on the parent is undone by the child.
    assert!(f
        .repo
        .runs(f.other_ws, made.id, 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_create_that_violates_a_check_constraint_leaves_nothing_behind() {
    // create writes three tables in one transaction; a rejected schedule must
    // not leave orphan owners or recipients.
    let f = fixture!();
    let mut broken = alert(&f, "no sql");
    broken.sql = None; // violates schedule_alert_needs_query

    let err = f.repo.create(f.ws, &broken, None).await.unwrap_err();
    assert!(
        matches!(err, egret_automation::AutomationError::BadRequest(_)),
        "expected BadRequest from the CHECK, got {err:?}"
    );

    let owners: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM automation.schedule_owners o \
         JOIN automation.schedules s ON s.id = o.schedule_id WHERE s.workspace_id = $1",
    )
    .bind(f.ws)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(owners, 0);
    assert!(f.repo.list(f.ws, None).await.unwrap().is_empty());
}

// ── helpers ───────────────────────────────────────────────────────────────

async fn other_ws_database(f: &Fixture) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id) \
         VALUES ($1,'trino','localhost',8080,$2) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(f.other_ws)
    .fetch_one(&f.db.pool)
    .await
    .unwrap()
}

/// A dashboard in the fixture workspace, for report schedules.
async fn a_dashboard(f: &Fixture) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO analytics.dashboards (workspace_id, name, created_by) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(f.ws)
    .bind(format!("d-{}", Uuid::new_v4().simple()))
    .bind(f.user)
    .fetch_one(&f.db.pool)
    .await
    .unwrap()
}
