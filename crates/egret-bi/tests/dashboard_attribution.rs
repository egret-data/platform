//! DB-gated attribution tests. Self-skip without DATABASE_URL.
use egret_bi::DashboardRepo;
use egret_metastore::Db;
use uuid::Uuid;

async fn setup() -> Option<(DashboardRepo, Db, Uuid, Uuid, Uuid)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    // One user WITH a profile name and one with only an email: the label rule has to prefer the
    // name when there is one and fall back to the email when there is not, and a test that used
    // only one of them would pass against either half of that rule.
    let named: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider) VALUES ($1,'Ref User','local') RETURNING id",
    )
    .bind(format!("named_{}@t.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let email = format!("bare_{}@t.local", Uuid::new_v4().simple());
    let nameless: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider) VALUES ($1,'','local') RETURNING id",
    )
    .bind(&email)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(named)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    Some((DashboardRepo::new(db.clone()), db, ws, named, nameless))
}

#[tokio::test]
async fn new_dashboards_stamp_created_at_and_updates_stamp_updated_by() {
    let Some((repo, _db, ws, named, nameless)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let d = repo.create(ws, "Ops", "", Some(named)).await.unwrap();
    assert!(
        d.created_at.is_some(),
        "0106's DEFAULT must stamp newly created rows"
    );
    assert!(
        d.updated_by.is_none(),
        "a fresh dashboard has no editor yet"
    );

    repo.update(
        d.id,
        "Ops",
        "edited",
        &serde_json::json!([]),
        "full",
        Some(nameless),
    )
    .await
    .unwrap();
    let after = repo.get(d.id, ws).await.unwrap().unwrap();
    assert_eq!(after.updated_by, Some(nameless));
    assert_eq!(
        after.created_at, d.created_at,
        "an edit must not move the creation date"
    );
}

#[tokio::test]
async fn attribution_names_prefer_the_profile_name_and_fall_back_to_the_email() {
    let Some((repo, db, ws, named, nameless)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let email: String = sqlx::query_scalar("SELECT email::text FROM users WHERE id = $1")
        .bind(nameless)
        .fetch_one(&db.pool)
        .await
        .unwrap();

    let d = repo.create(ws, "Ops2", "", Some(named)).await.unwrap();
    repo.update(
        d.id,
        "Ops2",
        "",
        &serde_json::json!([]),
        "full",
        Some(nameless),
    )
    .await
    .unwrap();

    let (created, updated) = repo.attribution_names(d.id).await.unwrap();
    assert_eq!(created.as_deref(), Some("Ref User"));
    assert_eq!(updated.as_deref(), Some(email.as_str()));
}

#[tokio::test]
async fn attribution_names_are_none_when_the_id_is_none() {
    let Some((repo, _db, ws, _named, _nameless)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let d = repo.create(ws, "Ops3", "", None).await.unwrap();
    let (created, updated) = repo.attribution_names(d.id).await.unwrap();
    assert_eq!(created, None);
    assert_eq!(updated, None, "never edited ⇒ no editor name");
}
