//! `perm.engine_grants` (migration 0195) and its store: round trips, the
//! table's `CHECK`s, and the two cascades (engine row, impersonation target).
//!
//! DB-gated; FAILS without a database unless `EGRET_ALLOW_DB_SKIP` is set:
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm --test engine_grants_store

use egret_metastore::Db;
use egret_perm::{
    create_engine_grant_in_tx, delete_engine_grant_in_tx, get_engine_grant_via,
    list_engine_grants_for_engine_via, list_engine_grants_via, update_engine_grant_in_tx,
    EngineGrant, Principal, PrincipalKind, PrincipalSet,
};
use sqlx::PgPool;
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..12])
}

async fn seed_engine(pool: &PgPool, ty: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec) \
         VALUES ($1, $2, 'egret', '{}'::jsonb) RETURNING id",
    )
    .bind(unique("eg-eng"))
    .bind(ty)
    .fetch_one(pool)
    .await
    .expect("engine")
}

async fn seed_user(pool: &PgPool) -> Uuid {
    let name = unique("eg-user");
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) \
         VALUES ('local', $1, 'Engine grant target', $2) RETURNING id",
    )
    .bind(format!("{name}@example.test"))
    .bind(&name)
    .fetch_one(pool)
    .await
    .expect("user")
}

fn principal(kind: PrincipalKind, r: &str) -> Principal {
    Principal {
        kind,
        r#ref: r.into(),
    }
}

fn grant(
    engine_id: Uuid,
    capability: &str,
    values: &[&str],
    principals: Vec<Principal>,
) -> EngineGrant {
    EngineGrant {
        id: Uuid::nil(),
        engine_id,
        capability: capability.into(),
        values: values.iter().map(|v| v.to_string()).collect(),
        target_user_id: None,
        principals: PrincipalSet {
            include: principals,
            exclude: vec![],
        },
    }
}

async fn create(pool: &PgPool, g: &EngineGrant) -> Uuid {
    let mut tx = pool.begin().await.expect("begin");
    let id = create_engine_grant_in_tx(&mut tx, g).await.expect("create");
    tx.commit().await.expect("commit");
    id
}

#[tokio::test]
async fn an_engine_grant_round_trips_updates_and_deletes() {
    let Some(db) = test_db().await else { return };
    let pool = db.pool.clone();
    let engine_id = seed_engine(&pool, "trino").await;

    let mut g = grant(
        engine_id,
        "trino.set_system_session_property",
        &["query_max_memory"],
        vec![
            principal(PrincipalKind::User, "alice"),
            principal(PrincipalKind::Group, "g1"),
        ],
    );
    let id = create(&pool, &g).await;
    g.id = id;

    let mut conn = pool.acquire().await.expect("conn");
    let back = get_engine_grant_via(&mut conn, id).await.expect("get");
    // Principals come back ordered by (kind, ref): `group` < `user`.
    let mut expected = g.clone();
    expected.principals.include.sort_by(|a, b| {
        (a.kind.as_str(), a.r#ref.as_str()).cmp(&(b.kind.as_str(), b.r#ref.as_str()))
    });
    assert_eq!(back, Some(expected.clone()));
    let listed = list_engine_grants_for_engine_via(&mut conn, engine_id)
        .await
        .expect("list for engine");
    assert_eq!(listed, vec![expected.clone()]);
    let all = list_engine_grants_via(&mut conn).await.expect("list all");
    assert!(all.contains(&expected));
    drop(conn);

    let updated = EngineGrant {
        values: vec!["query_max_run_time".into()],
        principals: PrincipalSet {
            include: vec![principal(PrincipalKind::Role, "r1")],
            exclude: vec![],
        },
        ..g.clone()
    };
    let mut tx = pool.begin().await.expect("begin");
    assert!(update_engine_grant_in_tx(&mut tx, &updated)
        .await
        .expect("update"));
    tx.commit().await.expect("commit");
    let mut conn = pool.acquire().await.expect("conn");
    assert_eq!(
        get_engine_grant_via(&mut conn, id).await.expect("get"),
        Some(updated.clone())
    );
    drop(conn);

    let mut tx = pool.begin().await.expect("begin");
    assert!(delete_engine_grant_in_tx(&mut tx, id)
        .await
        .expect("delete"));
    assert!(!delete_engine_grant_in_tx(&mut tx, id)
        .await
        .expect("delete again"));
    let missing = EngineGrant {
        id: Uuid::new_v4(),
        ..updated
    };
    assert!(!update_engine_grant_in_tx(&mut tx, &missing)
        .await
        .expect("update missing"));
    tx.commit().await.expect("commit");
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM perm.engine_grant_principals WHERE engine_grant_id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(left, 0, "principals cascade with the grant");
}

#[tokio::test]
async fn deleting_the_engine_deletes_its_grants_and_their_principals() {
    let Some(db) = test_db().await else { return };
    let pool = db.pool.clone();
    let engine_id = seed_engine(&pool, "starrocks").await;
    let id = create(
        &pool,
        &grant(
            engine_id,
            "starrocks.manage_blacklist",
            &[],
            vec![principal(PrincipalKind::Group, "ops")],
        ),
    )
    .await;

    sqlx::query("DELETE FROM engine_mgmt.engines WHERE id = $1")
        .bind(engine_id)
        .execute(&pool)
        .await
        .expect("delete engine");

    let grants: i64 = sqlx::query_scalar("SELECT count(*) FROM perm.engine_grants WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("count");
    let principals: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM perm.engine_grant_principals WHERE engine_grant_id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!((grants, principals), (0, 0));
}

/// The amended E21: the target is stored by `users.id`, and the grant goes with
/// the user, so a username freed by the delete can never carry it to the next
/// person minted that name.
#[tokio::test]
async fn an_impersonation_target_is_stored_by_identity_and_goes_with_the_user() {
    let Some(db) = test_db().await else { return };
    let pool = db.pool.clone();
    let engine_id = seed_engine(&pool, "trino").await;
    let target = seed_user(&pool).await;
    let mut g = grant(
        engine_id,
        "trino.impersonate_user",
        &[],
        vec![principal(PrincipalKind::User, "alice")],
    );
    g.target_user_id = Some(target);
    let id = create(&pool, &g).await;
    g.id = id;

    let mut conn = pool.acquire().await.expect("conn");
    assert_eq!(
        get_engine_grant_via(&mut conn, id).await.expect("get"),
        Some(g)
    );
    drop(conn);

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(target)
        .execute(&pool)
        .await
        .expect("delete user");
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM perm.engine_grants WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(left, 0, "the grant is deleted with its target user");
}

/// One raw insert of a grant row; `Err` carries the SQLSTATE.
async fn insert_raw(
    pool: &PgPool,
    engine_id: Uuid,
    capability: &str,
    values: Vec<Option<String>>,
    target: Option<Uuid>,
) -> Result<Uuid, String> {
    sqlx::query_scalar(
        "INSERT INTO perm.engine_grants (engine_id, capability, resource_values, target_user_id) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(engine_id)
    .bind(capability)
    .bind(values)
    .bind(target)
    .fetch_one(pool)
    .await
    .map_err(|e| {
        e.as_database_error()
            .and_then(|d| d.code())
            .map(|c| c.to_string())
            .unwrap_or_else(|| e.to_string())
    })
}

fn vals(v: &[&str]) -> Vec<Option<String>> {
    v.iter().map(|s| Some(s.to_string())).collect()
}

#[tokio::test]
async fn the_table_refuses_what_a_grant_cannot_mean() {
    let Some(db) = test_db().await else { return };
    let pool = db.pool.clone();
    let engine_id = seed_engine(&pool, "trino").await;
    let target = seed_user(&pool).await;
    let sys = "trino.set_system_session_property";

    /// `(why, capability, values, target)`.
    type Case<'a> = (&'a str, &'a str, Vec<Option<String>>, Option<Uuid>);
    let refused: Vec<Case> = vec![
        ("unknown capability", "trino.kill_any", vals(&["x"]), None),
        ("wildcard *", sys, vals(&["*"]), None),
        ("wildcard ?", sys, vals(&["a?b"]), None),
        ("macro", sys, vals(&["{USER}"]), None),
        (
            "catalog session property with one value",
            "trino.set_catalog_session_property",
            vals(&["ice"]),
            None,
        ),
        (
            "blacklist with a value",
            "starrocks.manage_blacklist",
            vals(&["x"]),
            None,
        ),
        ("NULL element", sys, vec![None], None),
        (
            "impersonation with no target",
            "trino.impersonate_user",
            vals(&[]),
            None,
        ),
        (
            "impersonation target given by name",
            "trino.impersonate_user",
            vals(&["bob"]),
            Some(target),
        ),
        (
            "a target on a capability that has none",
            sys,
            vals(&["query_max_memory"]),
            Some(target),
        ),
    ];
    for (why, cap, values, tgt) in refused {
        assert_eq!(
            insert_raw(&pool, engine_id, cap, values, tgt)
                .await
                .err()
                .as_deref(),
            Some("23514"),
            "{why} must be a check violation"
        );
    }

    let id = insert_raw(&pool, engine_id, sys, vals(&["query_max_memory"]), None)
        .await
        .expect("a valid row inserts");
    assert_eq!(
        insert_raw(&pool, engine_id, sys, vals(&["query_max_memory"]), None)
            .await
            .err()
            .as_deref(),
        Some("23505"),
        "a duplicate (engine, capability, values) is a unique violation"
    );
    insert_raw(
        &pool,
        engine_id,
        "trino.impersonate_user",
        vals(&[]),
        Some(target),
    )
    .await
    .expect("a target row inserts");
    assert_eq!(
        insert_raw(
            &pool,
            engine_id,
            "trino.impersonate_user",
            vals(&[]),
            Some(target)
        )
        .await
        .err()
        .as_deref(),
        Some("23505"),
        "a duplicate impersonation target is a unique violation"
    );
    insert_raw(
        &pool,
        engine_id,
        "starrocks.manage_blacklist",
        vals(&[]),
        None,
    )
    .await
    .expect("blacklist inserts");
    assert_eq!(
        insert_raw(
            &pool,
            engine_id,
            "starrocks.manage_blacklist",
            vals(&[]),
            None
        )
        .await
        .err()
        .as_deref(),
        Some("23505"),
        "NULL targets are not distinct: a second blacklist grant is a duplicate"
    );

    for (why, kind, r) in [
        (
            "workspace_members kind",
            "workspace_members",
            "00000000-0000-0000-0000-000000000000",
        ),
        ("the public group", "group", "public"),
        ("an empty ref", "user", ""),
    ] {
        let err = sqlx::query(
            "INSERT INTO perm.engine_grant_principals (engine_grant_id, kind, ref) \
             VALUES ($1, $2::perm.principal_kind, $3)",
        )
        .bind(id)
        .bind(kind)
        .bind(r)
        .execute(&pool)
        .await
        .expect_err(why);
        assert_eq!(
            err.as_database_error().and_then(|d| d.code()).as_deref(),
            Some("23514"),
            "{why} must be a check violation"
        );
    }
}
