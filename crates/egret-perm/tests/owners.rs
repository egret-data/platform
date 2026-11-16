//! One OWNER per securable (SP5a, concepts spec D4). DB-gated like `store.rs`.

use egret_metastore::Db;
use egret_perm::{get_owner_via, set_owner_via, Principal, PrincipalKind, Scope};
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn seed_catalog(pool: &sqlx::PgPool) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) \
         VALUES ($1, 'generic', '{}'::jsonb) RETURNING id",
    )
    .bind(format!(
        "own_{}",
        &Uuid::new_v4().simple().to_string()[..12]
    ))
    .fetch_one(pool)
    .await
    .expect("catalog")
}

fn scope(catalog_id: Uuid, schema: Option<&str>, table: Option<&str>) -> Scope {
    Scope {
        catalog_id,
        schema: schema.map(Into::into),
        table: table.map(Into::into),
        column: None,
    }
}

fn user(r: &str) -> Principal {
    Principal {
        kind: PrincipalKind::User,
        r#ref: r.into(),
    }
}

#[tokio::test]
async fn a_securable_has_exactly_one_owner() {
    let Some(db) = test_db().await else { return };
    let cat = seed_catalog(&db.pool).await;
    let s = scope(cat, Some("hr"), Some("people"));
    let mut conn = db.pool.acquire().await.unwrap();
    set_owner_via(&mut conn, &s, &user("alice")).await.unwrap();

    // A second raw INSERT for the same securable is refused by the database,
    // including at the catalog level where schema/table/column are all NULL.
    for sc in [s.clone(), scope(cat, None, None)] {
        if sc.schema.is_none() {
            set_owner_via(&mut conn, &sc, &user("alice")).await.unwrap();
        }
        let dup = sqlx::query(
            "INSERT INTO perm.securable_owners \
             (catalog_id, schema_name, table_name, principal_kind, principal_ref) \
             VALUES ($1, $2, $3, 'user', 'mallory')",
        )
        .bind(sc.catalog_id)
        .bind(&sc.schema)
        .bind(&sc.table)
        .execute(&mut *conn)
        .await;
        assert!(dup.is_err(), "second owner accepted for {sc:?}");
    }
    // A sibling securable is independent.
    set_owner_via(&mut conn, &scope(cat, Some("hr"), None), &user("bob"))
        .await
        .unwrap();
    assert_eq!(
        get_owner_via(&mut conn, &s)
            .await
            .unwrap()
            .unwrap()
            .principal,
        user("alice")
    );
}

#[tokio::test]
async fn transfer_changes_the_holder_not_the_row() {
    let Some(db) = test_db().await else { return };
    let cat = seed_catalog(&db.pool).await;
    let s = scope(cat, Some("hr"), None);
    let mut conn = db.pool.acquire().await.unwrap();
    let first = set_owner_via(&mut conn, &s, &user("alice")).await.unwrap();
    let second = set_owner_via(&mut conn, &s, &user("bob")).await.unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(second.principal, user("bob"));
    assert_eq!(
        get_owner_via(&mut conn, &s)
            .await
            .unwrap()
            .unwrap()
            .principal,
        user("bob")
    );
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM perm.securable_owners WHERE catalog_id = $1")
            .bind(cat)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(n, 1);
}
