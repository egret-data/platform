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
    let url = egret_test_db::url().await?;
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
        repo: ScheduleRepo::new(
            db.clone(),
            // Crypto-enabled: `SecretStore::new(db, None)` cannot ENCRYPT, so a
            // webhook secret would fail to vault on write.
            egret_secrets::SecretStore::new(
                db.clone(),
                Some(egret_secrets::Crypto::new(&[9u8; 32]).unwrap()),
            ),
        ),
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
        dataapi_endpoint_id: None,
        dataapi_metric: None,
        dataapi_error_code: None,
        dataapi_window_s: None,
        grace_period_s: 14400,
        working_timeout_s: 3600,
        log_retention_days: 90,
        email_subject: None,
        message: None,
        message_footer: None,
        skip_if_empty: false,
        targets: vec![],
        owners: vec![f.user],
        recipients: vec![(
            "email".into(),
            serde_json::json!({"addresses": ["on-call@t.local"]}),
        )],
    }
}

/// A valid `dataapi_alert`, the minimum that satisfies
/// `schedule_dataapi_needs_source` — workspace-scoped
/// (`dataapi_endpoint_id: None`).
fn dataapi_alert(f: &Fixture, name: &str) -> NewSchedule {
    NewSchedule {
        r#type: "dataapi_alert".into(),
        name: name.into(),
        description: String::new(),
        active: true,
        crontab: "*/5 * * * *".into(),
        timezone: "UTC".into(),
        database_id: None,
        sql: None,
        validator_type: Some("operator".into()),
        validator_config: serde_json::json!({"op": ">", "threshold": 0}),
        chart_id: None,
        dashboard_id: None,
        report_format: None,
        custom_width: None,
        custom_height: None,
        dataapi_endpoint_id: None,
        dataapi_metric: Some("call_count".into()),
        dataapi_error_code: None,
        dataapi_window_s: Some(300),
        grace_period_s: 14400,
        working_timeout_s: 3600,
        log_retention_days: 90,
        email_subject: None,
        message: None,
        message_footer: None,
        skip_if_empty: false,
        targets: vec![],
        owners: vec![f.user],
        recipients: vec![],
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

    let mine = f.repo.list(f.ws, None, f.user, false).await.unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].0.workspace_id, f.ws);
}

#[tokio::test]
async fn list_filters_by_type_for_the_two_tabs() {
    let f = fixture!();
    f.repo
        .create(f.ws, &alert(&f, "an alert"), None)
        .await
        .unwrap();

    let alerts = f
        .repo
        .list(f.ws, Some("alert"), f.user, false)
        .await
        .unwrap();
    assert_eq!(alerts.len(), 1);
    let reports = f
        .repo
        .list(f.ws, Some("report"), f.user, false)
        .await
        .unwrap();
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
    // Every minute, so the recomputed value must land on the NEXT minute
    // boundary — within 60s of now, whatever `before` happened to be.
    changed.crontab = "* * * * *".into();
    let sent_at = chrono::Utc::now();
    let after = f
        .repo
        .update(f.ws, made.id, &changed, None)
        .await
        .unwrap()
        .expect("updated")
        .next_run_at
        .expect("still set");

    // NOT `after < before`. That assertion failed for exactly one minute a day
    // and did so on 2026-08-13: the fixture's `0 9 * * *` gives 09:00:00, and
    // at 08:59:xx "every minute" gives 09:00:00 too, so the two were equal and
    // the run was red for a reason that had nothing to do with the change
    // under test. Assert the property the update actually promises — the new
    // expression governs — rather than an inequality that is only usually a
    // consequence of it.
    assert!(
        after > sent_at && after <= sent_at + chrono::Duration::seconds(61),
        "the new crontab must govern: {after} is not within a minute of {sent_at} \
         (before was {before})"
    );
}

/// Fix-round-1, Important 2: nothing before this test ever bound the four new
/// `dataapi_*` columns to anything but `NULL` — `dataapi_alert()`'s create
/// path binds real values, but no UPDATE test touched them at all, so a
/// swapped `$18`-`$21` bind (e.g. `dataapi_metric`/`dataapi_error_code`
/// transposed) would have been invisible. This uses four DISTINCT,
/// order-sensitive values on the UPDATE path specifically, and reads every
/// one back from Postgres by column NAME (`ScheduleRepo::get`'s `SELECT
/// {COLS}` + `sqlx::FromRow`) — a transposed bind would put the wrong VALUE
/// in the right-named column, which this test would catch; only a
/// same-shaped, same-adjacent transposition of two `Option<String>` columns
/// could hide from it, which is exactly why `dataapi_metric` and
/// `dataapi_error_code` (the two adjacent, same-typed columns) are given
/// values that could not be confused for one another if swapped.
#[tokio::test]
async fn updating_a_dataapi_alert_round_trips_all_four_new_fields() {
    let f = fixture!();
    let made = f
        .repo
        .create(f.ws, &dataapi_alert(&f, "traffic"), None)
        .await
        .unwrap();
    assert!(
        made.dataapi_endpoint_id.is_none(),
        "starts workspace-scoped"
    );
    assert_eq!(made.dataapi_metric.as_deref(), Some("call_count"));
    assert!(made.dataapi_error_code.is_none());
    assert_eq!(made.dataapi_window_s, Some(300));

    // A real endpoint to repoint to — proves the FK-backed column, not an
    // opaque uuid nothing else references.
    let endpoint_id: Uuid = sqlx::query_scalar(
        "INSERT INTO dataapi.endpoints \
           (workspace_id, name, path, statements, database_id, created_by) \
         VALUES ($1,'traffic endpoint','/traffic','[{\"name\":\"main\",\"sql\":\"SELECT 1\"}]',$2,$3) RETURNING id",
    )
    .bind(f.ws)
    .bind(f.database)
    .bind(f.user)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();

    let mut changed = dataapi_alert(&f, "traffic");
    changed.dataapi_endpoint_id = Some(endpoint_id);
    changed.dataapi_metric = Some("error_code_count".into());
    changed.dataapi_error_code = Some("QUERY_TIMEOUT".into());
    changed.dataapi_window_s = Some(900);

    f.repo
        .update(f.ws, made.id, &changed, None)
        .await
        .unwrap()
        .expect("updated");

    let got = f.repo.get(f.ws, made.id).await.unwrap().expect("present");
    assert_eq!(
        got.dataapi_endpoint_id,
        Some(endpoint_id),
        "the endpoint must be repointed, not left in another column's slot"
    );
    assert_eq!(
        got.dataapi_metric.as_deref(),
        Some("error_code_count"),
        "must not have landed in dataapi_error_code's column instead"
    );
    assert_eq!(
        got.dataapi_error_code.as_deref(),
        Some("QUERY_TIMEOUT"),
        "must not have landed in dataapi_metric's column instead"
    );
    assert_eq!(got.dataapi_window_s, Some(900));
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
        // AssertSqlSafe: `table` iterates the literal array declared just above.
        let left: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM {table} WHERE schedule_id = $1"
        )))
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
    assert!(f
        .repo
        .list(f.ws, None, f.user, false)
        .await
        .unwrap()
        .is_empty());
}

/// Fix-round-1, Minor 4: `dataapi_endpoint_id` naming a uuid that is not a
/// real `dataapi.endpoints` row hits `schedules_dataapi_endpoint_id_fkey`
/// (a plain FK, not one of the four CHECKs `to_new` mirrors) — before
/// `AutomationError::from_db` learned this constraint's name, that surfaced
/// as an opaque 500 rather than a 400 naming the field.
#[tokio::test]
async fn a_nonexistent_dataapi_endpoint_id_is_a_400_not_a_500() {
    let f = fixture!();
    let mut broken = dataapi_alert(&f, "ghost endpoint");
    broken.dataapi_endpoint_id = Some(Uuid::new_v4()); // no such row

    let err = f.repo.create(f.ws, &broken, None).await.unwrap_err();
    match err {
        egret_automation::AutomationError::BadRequest(m) => {
            assert!(
                m.contains("dataapi_endpoint_id"),
                "expected the message to name the field: {m}"
            );
        }
        other => panic!("expected BadRequest naming dataapi_endpoint_id, got {other:?}"),
    }
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

/// A vaulted webhook secret must come back out as the SAME plaintext.
///
/// This is the seam the runner signs with: `vault_secrets` stores an id and
/// `resolve_webhook_secret` turns it back into the key the HMAC is computed
/// from. If the round trip breaks, every webhook silently delivers unsigned —
/// a receiver rejects it, but nothing in egret would have said so. The rest of
/// the chain is already covered: `webhook::sign` has its own digest tests and
/// `deliver` sets the header iff it is given a secret.
#[tokio::test]
async fn a_vaulted_webhook_secret_round_trips_to_the_signing_key() {
    const SECRET: &str = "the-key-the-hmac-uses";
    let f = fixture!();

    let mut n = alert(&f, "signed-hook");
    n.recipients = vec![(
        "webhook".into(),
        serde_json::json!({"url": "https://hooks.example.com/x", "secret": SECRET}),
    )];
    let made = f.repo.create(f.ws, &n, None).await.expect("create");

    let stored = f.repo.recipients_for(made.id).await.expect("recipients");
    let hook = stored
        .iter()
        .find(|r| r.kind == "webhook")
        .expect("webhook");
    // Stored as a reference, not a value...
    assert!(
        hook.config.get("secret").is_none(),
        "cleartext secret at rest: {}",
        hook.config
    );
    assert!(hook.config.get("secret_id").is_some());

    // ...and the reference resolves back to the original key.
    assert_eq!(
        f.repo.resolve_webhook_secret(&hook.config).await.as_deref(),
        Some(SECRET),
    );
}

/// Rotating a secret must not leave the superseded one in the vault.
#[tokio::test]
async fn rotating_a_webhook_secret_deletes_the_old_vault_row() {
    let f = fixture!();
    let mut n = alert(&f, "rotate-hook");
    n.recipients = vec![(
        "webhook".into(),
        serde_json::json!({"url": "https://hooks.example.com/x", "secret": "first"}),
    )];
    let made = f.repo.create(f.ws, &n, None).await.expect("create");
    let before = f.repo.recipients_for(made.id).await.unwrap();
    let old_id = before[0].config["secret_id"].as_str().unwrap().to_string();

    n.recipients = vec![(
        "webhook".into(),
        serde_json::json!({"url": "https://hooks.example.com/x", "secret": "second"}),
    )];
    f.repo
        .update(f.ws, made.id, &n, None)
        .await
        .expect("update")
        .expect("still there");

    let after = f.repo.recipients_for(made.id).await.unwrap();
    let new_id = after[0].config["secret_id"].as_str().unwrap().to_string();
    assert_ne!(old_id, new_id, "rotation should mint a new vault row");
    assert_eq!(
        f.repo
            .resolve_webhook_secret(&after[0].config)
            .await
            .as_deref(),
        Some("second"),
    );

    let orphan: i64 = sqlx::query_scalar("SELECT count(*) FROM secrets WHERE id = $1::uuid")
        .bind(&old_id)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    assert_eq!(orphan, 0, "the superseded secret is still in the vault");
}

/// A report with several targets keeps them, in the order given.
///
/// Page order is not cosmetic: a report that reordered itself between runs
/// makes two copies impossible to compare side by side.
#[tokio::test]
async fn a_report_keeps_its_targets_in_page_order() {
    let f = fixture!();
    let mut n = alert(&f, "multi");
    n.r#type = "report".into();
    n.sql = None;
    n.database_id = None;
    n.validator_type = None;
    n.report_format = Some("pdf".into());
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    n.targets = vec![("dashboard".into(), a), ("dashboard".into(), b)];

    let made = f.repo.create(f.ws, &n, None).await.expect("create");
    let got = f.repo.targets_for(made.id).await.expect("targets");
    assert_eq!(
        got.iter().map(|t| t.object_id).collect::<Vec<_>>(),
        vec![a, b],
        "targets must come back in the order they were given"
    );
    assert_eq!(got[0].ordinal, 0);
    assert_eq!(got[1].ordinal, 1);
}

/// An update REPLACES the target list rather than appending to it.
#[tokio::test]
async fn updating_a_report_replaces_its_targets() {
    let f = fixture!();
    let mut n = alert(&f, "replace-targets");
    n.r#type = "report".into();
    n.sql = None;
    n.database_id = None;
    n.validator_type = None;
    n.report_format = Some("pdf".into());
    n.targets = vec![("dashboard".into(), Uuid::new_v4())];
    let made = f.repo.create(f.ws, &n, None).await.expect("create");

    let kept = Uuid::new_v4();
    n.targets = vec![("chart".into(), kept)];
    f.repo
        .update(f.ws, made.id, &n, None)
        .await
        .expect("update")
        .expect("still there");

    let got = f.repo.targets_for(made.id).await.unwrap();
    assert_eq!(got.len(), 1, "the old target must not survive: {got:?}");
    assert_eq!(got[0].object_id, kept);
}
