//! #792 (username-takeover spec 2026-10-23 §11 item 6; P6 spec
//! `docs/en/specs/2026-11-01-p6-compile-mint-ordering.md`): the profile set a
//! writer locks before the compile-mint lock can GROW before its recompile, and
//! the grown profile is then locked while the mint lock is held, against the
//! stated order (profiles, then the mint lock, then any row write or mint).
//!
//! Driven as a deterministic interleaving of three sessions over the real
//! catalog-tag reach (`ViewReach`, which `egret_bi::CatalogTagRepo` and the
//! table hook's move both use):
//!
//! 1. A: `ViewReach::of_tables` on warehouse W. W's only bound profile is S1,
//!    so A locks S1 and then the compile-mint lock.
//! 2. B: attaches the engine of profile S2 to a catalog of W, and commits. W
//!    is now reached through S1 and S2.
//! 3. C: a writer of S2 in the stated order: the S2 profile lock, then the
//!    mint lock, which A holds, so C waits.
//! 4. A: `retag_bump_and_recompile` reads W's profiles again, finds S2, and
//!    locks it while holding the mint lock. A waits on C, C waits on A.
//!
//! Wanted (owner decision for #792, option (a)): A refuses with a retryable
//! error before it waits on any profile it did not lock early, and C goes
//! through. On the code before the fix Postgres breaks the cycle with a
//! deadlock (40P01), and C is the victim.
//!
//! Its own test binary, so `egret-test-db` gives it a private database.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test compile_mint_ordering

use std::time::{Duration, Instant};

use egret_metastore::Db;
use egret_perm::CompileError;
use egret_perm_compile::view_tags::{ViewReach, ViewReachError};
use egret_policy::PolicyStore;
use uuid::Uuid;

/// Every test here recompiles, and a recompile mints every unnamed person the
/// model names (installation-wide), so the tests run one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A backstop so a regression hangs for this long at most, not forever. Far
/// above Postgres' `deadlock_timeout` (1 s by default), so the deadlock, when
/// there is one, is what ends the wait.
const LOCK_TIMEOUT: &str = "20s";

async fn test_db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 6).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Profile {
    service_id: i64,
    engine_id: Uuid,
}

/// A StarRocks profile with one engine bound to it (StarRocks, so nothing here
/// needs a tag service).
async fn profile(db: &Db, policy: &PolicyStore) -> Profile {
    let service = policy
        .create_service(&unique("sr_mintorder"), "starrocks", &serde_json::json!({}))
        .await
        .expect("create service");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("mintorder-eng"))
    .bind(service.id)
    .fetch_one(&db.pool)
    .await
    .expect("engine");
    Profile {
        service_id: service.id,
        engine_id,
    }
}

async fn warehouse(db: &Db) -> Uuid {
    let pool = &db.pool;
    let workspace_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("mintorder-ws"))
            .bind("Mint Order WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-mintorder-test','AKIATEST') RETURNING id",
    )
    .bind(unique("mintorder-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("mintorder_wh"))
    .bind(workspace_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse")
}

async fn catalog(db: &Db, warehouse_id: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(unique("mintorder_cat"))
    .bind(warehouse_id)
    .fetch_one(&db.pool)
    .await
    .expect("registry catalog")
}

async fn attach(db: &Db, engine_id: Uuid, catalog_id: Uuid) {
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(&db.pool)
    .await
    .expect("attach catalog");
}

async fn begin_bounded(db: &Db) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut tx = db.pool.begin().await.expect("begin");
    sqlx::query("SELECT set_config('lock_timeout', $1, true)")
        .bind(LOCK_TIMEOUT)
        .execute(&mut *tx)
        .await
        .expect("lock_timeout");
    tx
}

/// Wait until some session waits on an advisory lock (C on the mint lock).
async fn until_waiting_on_advisory_lock(db: &Db) {
    let start = Instant::now();
    loop {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_locks \
                             WHERE locktype = 'advisory' AND NOT granted \
                               AND database = (SELECT oid FROM pg_database \
                                                WHERE datname = current_database()))",
        )
        .fetch_one(&db.pool)
        .await
        .expect("pg_locks");
        if waiting {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "C never reached its wait on the compile-mint lock"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn is_deadlock(e: &str) -> bool {
    e.contains("40P01") || e.contains("deadlock")
}

#[tokio::test]
async fn a_profile_bound_after_the_early_mint_lock_is_refused_not_deadlocked() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else {
        return;
    };
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks service-def");

    // S1 is created first, so S1 < S2: A's early lock (S1) and the grown one
    // (S2) are ascending, and the only order broken is the mint lock's.
    let s1 = profile(&db, &policy).await;
    let s2 = profile(&db, &policy).await;
    assert!(s1.service_id < s2.service_id);
    let w = warehouse(&db).await;
    let c1 = catalog(&db, w).await;
    attach(&db, s1.engine_id, c1).await;

    // 1. A: the reach of a catalog-tag write on W.
    let mut a = begin_bounded(&db).await;
    let reach = ViewReach::of_tables(&mut a, w, &[("sales".into(), "orders".into())])
        .await
        .expect("reach");
    assert_eq!(
        reach.services,
        vec![s1.service_id],
        "W is reached through S1 only"
    );

    // 2. B: W gains a second profile, committed.
    attach(&db, s2.engine_id, c1).await;

    // 3. C: a writer of S2, in the stated order; it waits on A's mint lock.
    let c_db = db.clone();
    let s2_id = s2.service_id;
    let c = tokio::spawn(async move {
        let mut tx = begin_bounded(&c_db).await;
        egret_perm_compile::lock_services_in_tx(&mut tx, &[s2_id])
            .await
            .map_err(|e| e.to_string())?;
        egret_perm_compile::lock_compile_mints_in_tx(&mut tx)
            .await
            .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())
    });
    until_waiting_on_advisory_lock(&db).await;

    // 4. A: the re-read finds S2.
    let started = Instant::now();
    let a_result = reach.retag_bump_and_recompile(&mut a).await;
    let a_took = started.elapsed();
    let a_outcome: Result<(), ViewReachError> = match a_result {
        Ok(()) => a.commit().await.map_err(ViewReachError::Db),
        Err(e) => {
            let _ = a.rollback().await;
            Err(e)
        }
    };
    let c_outcome = tokio::time::timeout(Duration::from_secs(40), c)
        .await
        .expect("C finished")
        .expect("C did not panic");

    // A refuses with the retryable refusal naming S2, without waiting on it
    // (a deadlock is only detected after `deadlock_timeout`, 1 s).
    match a_outcome {
        Err(ViewReachError::Compile(CompileError::ProfileSetChanged { added })) => {
            assert_eq!(
                added,
                vec![s2.service_id],
                "the refusal names the grown profile"
            );
        }
        Ok(()) => panic!(
            "A recompiled S2, a profile it locked AFTER the compile-mint lock; C's outcome: \
             {c_outcome:?}"
        ),
        Err(other) => panic!(
            "A failed{} instead of refusing with ProfileSetChanged: {other:?}",
            if is_deadlock(&format!("{other:?}")) {
                " as a deadlock victim"
            } else {
                ""
            }
        ),
    }
    assert!(
        a_took < Duration::from_millis(900),
        "A refused only after {a_took:?}: it waited on S2"
    );
    // C, which kept the stated order, goes through.
    c_outcome.unwrap_or_else(|e| {
        panic!(
            "C (profile lock, then mint lock) failed{}: {e}",
            if is_deadlock(&e) {
                " as a deadlock victim"
            } else {
                ""
            }
        )
    });
}

/// The guard itself, through the public entry points: under the mint lock a
/// profile already locked is re-taken, a new one is refused at once (even
/// while another session holds its row), and without the mint lock a new
/// profile is locked as before.
#[tokio::test]
async fn under_the_mint_lock_only_profiles_locked_first_can_be_locked() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else {
        return;
    };
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_starrocks(&policy).await.expect("def");
    let s1 = profile(&db, &policy).await.service_id;
    let s2 = profile(&db, &policy).await.service_id;

    // Another session holds S2's row for the whole test.
    let mut holder = begin_bounded(&db).await;
    sqlx::query("SELECT id FROM policy.authz_services WHERE id = $1 FOR UPDATE")
        .bind(s2)
        .execute(&mut *holder)
        .await
        .expect("hold S2");

    let mut tx = begin_bounded(&db).await;
    egret_perm_compile::lock_services_in_tx(&mut tx, &[s1])
        .await
        .expect("S1 before the mint lock");
    egret_perm_compile::lock_compile_mints_in_tx(&mut tx)
        .await
        .expect("mint lock");
    egret_perm_compile::lock_services_in_tx(&mut tx, &[s1])
        .await
        .expect("a re-take of S1 under the mint lock");
    let started = Instant::now();
    let refused = egret_perm_compile::lock_services_in_tx(&mut tx, &[s1, s2]).await;
    assert!(
        matches!(&refused, Err(CompileError::ProfileSetChanged { added }) if added == &vec![s2]),
        "{refused:?}"
    );
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "the refusal waited on S2's holder"
    );
    let body = refused
        .unwrap_err()
        .profile_set_changed_body()
        .expect("coded body");
    assert_eq!(body["code"], "profile_set_changed");
    assert_eq!(body["retryable"], true);
    tx.rollback().await.expect("rollback");

    // Without the mint lock, a new profile is locked as before (it waits for
    // the holder, so release it first).
    holder.rollback().await.expect("release S2");
    let mut tx = begin_bounded(&db).await;
    egret_perm_compile::lock_services_in_tx(&mut tx, &[s1])
        .await
        .expect("S1");
    egret_perm_compile::lock_services_in_tx(&mut tx, &[s1, s2])
        .await
        .expect("S2 without the mint lock");
    tx.rollback().await.expect("rollback");
}

/// #791 fixtures: a person the model names (through a clearance's required
/// role) who has no username yet, so a pre-mint sweep would mint them.
async fn unnamed_clearance_holder(db: &Db) -> Uuid {
    let user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local',$1,'T') RETURNING id",
    )
    .bind(format!("{}@p6.test", unique("u")))
    .fetch_one(&db.pool)
    .await
    .expect("user");
    let role = unique("p6_role");
    let role_id: Uuid =
        sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
            .bind(&role)
            .fetch_one(&db.pool)
            .await
            .expect("role");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
         VALUES ('user', $1, $2, 'platform')",
    )
    .bind(user)
    .bind(role_id)
    .execute(&db.pool)
    .await
    .expect("role grant");
    let tag_id: Uuid =
        sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $1) RETURNING id")
            .bind(unique("p6_tag"))
            .fetch_one(&db.pool)
            .await
            .expect("tag");
    sqlx::query("INSERT INTO classification_policies (tag_id, required_roles) VALUES ($1, $2)")
        .bind(tag_id)
        .bind(vec![role])
        .execute(&db.pool)
        .await
        .expect("clearance");
    user
}

async fn username_of(db: &Db, user: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
        .bind(user)
        .fetch_one(&db.pool)
        .await
        .expect("username")
}

/// Whether THIS transaction holds the compile-mint lock, read from
/// `pg_locks` (not from the marker the code sets).
async fn holds_mint_lock(tx: &mut sqlx::Transaction<'static, sqlx::Postgres>) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_locks l \
                         WHERE l.locktype = 'advisory' AND l.pid = pg_backend_pid() \
                           AND l.granted \
                           AND ((l.classid::bigint << 32) | l.objid::bigint) \
                               = hashtextextended('egret-compile-mint', 0))",
    )
    .fetch_one(&mut **tx)
    .await
    .expect("pg_locks")
}

async fn tag_service(db: &Db, policy: &PolicyStore) -> i64 {
    egret_policy::seed_tag(policy).await.expect("tag def");
    let _ = db;
    policy
        .create_service(&unique("p6_tagsvc"), "tag", &serde_json::json!({}))
        .await
        .expect("tag service")
        .id
}

/// #791: a recompile that cannot mint (a tag service: the compiler targets
/// only trino and starrocks, and it holds no `egret-admin` set) runs no
/// pre-mint, so it takes no compile-mint lock and mints nobody.
#[tokio::test]
async fn a_recompile_that_cannot_mint_runs_no_premint() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else {
        return;
    };
    let policy = PolicyStore::new(db.clone());
    let user = unnamed_clearance_holder(&db).await;
    let svc = tag_service(&db, &policy).await;

    let mut tx = begin_bounded(&db).await;
    egret_perm_compile::recompile_service_in_tx(&mut tx, svc)
        .await
        .expect("recompile");
    assert!(
        !holds_mint_lock(&mut tx).await,
        "a recompile with nothing to compile took the compile-mint lock"
    );
    tx.commit().await.expect("commit");
    assert_eq!(username_of(&db, user).await, None, "nobody was minted");
}

/// The instrument of the test above, and the targeted case: a StarRocks
/// profile's recompile still runs the sweep, mints the clearance holder and
/// takes the mint lock.
#[tokio::test]
async fn a_recompile_of_a_targeted_profile_still_premints() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else {
        return;
    };
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_starrocks(&policy).await.expect("def");
    let user = unnamed_clearance_holder(&db).await;
    let svc = profile(&db, &policy).await.service_id;

    let mut tx = begin_bounded(&db).await;
    egret_perm_compile::recompile_service_in_tx(&mut tx, svc)
        .await
        .expect("recompile");
    assert!(
        holds_mint_lock(&mut tx).await,
        "the sweep minted under the lock"
    );
    tx.commit().await.expect("commit");
    assert!(
        username_of(&db, user).await.is_some(),
        "the sweep minted the holder"
    );
}

/// #791's egret-admin guard: a service the compiler does not target still
/// syncs its `egret-admin` set, which can mint, so its recompile keeps the
/// sweep (the clearance holder, whom the admin sync alone never names, is
/// minted).
#[tokio::test]
async fn a_service_with_an_egret_admin_set_keeps_the_premint() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else {
        return;
    };
    let policy = PolicyStore::new(db.clone());
    let user = unnamed_clearance_holder(&db).await;
    let svc = tag_service(&db, &policy).await;
    let name: String = sqlx::query_scalar("SELECT name FROM policy.authz_services WHERE id = $1")
        .bind(svc)
        .fetch_one(&db.pool)
        .await
        .expect("name");
    egret_policy::seed_trino(&policy).await.expect("trino def");
    let n = policy
        .seed_admin_all_access(svc, &name, "trino", &["p6-seeded-admin".to_string()])
        .await
        .expect("seed egret-admin");
    assert!(n > 0, "the service holds an egret-admin set");

    let mut tx = begin_bounded(&db).await;
    egret_perm_compile::recompile_service_in_tx(&mut tx, svc)
        .await
        .expect("recompile");
    tx.commit().await.expect("commit");
    assert!(
        username_of(&db, user).await.is_some(),
        "the sweep ran for a service whose egret-admin sync can mint"
    );
}

/// A recompile bumps its tag service's `policy_version`, and migration 0191's
/// trigger carries the bump to every profile naming that tag service, taking
/// their row locks. So `lock_services_in_tx` locks those linkers up front: a
/// second profile sharing the tag service is held after locking the first.
#[tokio::test]
async fn locking_a_profile_also_locks_the_other_linkers_of_its_tag_service() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else {
        return;
    };
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("trino def");
    let tag_name = unique("p6_shared_tags");
    egret_policy::seed_tag(&policy).await.expect("tag def");
    let tag = policy
        .create_service(&tag_name, "tag", &serde_json::json!({}))
        .await
        .expect("tag service")
        .id;
    let config = serde_json::json!({ "tagService": tag_name });
    let p1 = policy
        .create_service(&unique("p6_linker"), "trino", &config)
        .await
        .expect("profile 1")
        .id;
    let p2 = policy
        .create_service(&unique("p6_linker"), "trino", &config)
        .await
        .expect("profile 2")
        .id;

    let mut tx = begin_bounded(&db).await;
    egret_perm_compile::lock_services_in_tx(&mut tx, &[p1])
        .await
        .expect("lock profile 1");
    let mut other = db.pool.begin().await.expect("begin");
    for (what, id) in [
        ("profile 1", p1),
        ("its tag service", tag),
        ("profile 2", p2),
    ] {
        let r = sqlx::query("SELECT id FROM policy.authz_services WHERE id = $1 FOR UPDATE NOWAIT")
            .bind(id)
            .execute(&mut *other)
            .await;
        assert!(r.is_err(), "{what} ({id}) was not locked");
        let _ = other.rollback().await;
        other = db.pool.begin().await.expect("begin");
    }
    tx.rollback().await.expect("rollback");
}
