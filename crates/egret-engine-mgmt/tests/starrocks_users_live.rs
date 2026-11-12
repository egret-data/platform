//! Engine-gated: the StarRocks user sync against a real FE (package D,
//! Task 12). Self-skips unless `EGRET_TEST_STARROCKS_URL` (egret's
//! `4.1.7-egret.4` allin1 image; root has no password) is set, so it reports
//! `ok` while asserting nothing: the DURATION tells a pass from a skip.
//!
//! The store is the test database (`DATABASE_URL`, through `egret-test-db`);
//! the FE is reached as `root`. The platform admins of the test database are
//! synced too, so every assertion names this test's own users only. The test
//! drops what it created on the FE and in the database, also when an assertion
//! fails.
//!
//! Run with (the container already up):
//!   env -u EGRET_ALLOW_DB_SKIP \
//!     DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!     EGRET_TEST_STARROCKS_URL=mysql://127.0.0.1:19330 \
//!     cargo test -p egret-engine-mgmt --features test-util \
//!       --test starrocks_users_live -- --nocapture

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

use egret_engine_mgmt::starrocks_users_live_testing::{
    self as su, show_delegator_grants_sql, UserSync, SHOW_USERS_SQL,
};
use egret_engine_mgmt::{Engine, EngineMgmtState, FeError, FeTarget, MysqlDdlRunner, StarRocksDdl};
use egret_query_api::{quote_string, Engine as QueryEngine};
use mysql_async::prelude::Queryable;
use uuid::Uuid;

const DELEGATOR: &str = "egret_delegator";
const DELEGATOR_PASSWORD: &str = "delegator-pw-live";

fn fe_from(var: &str) -> Option<FeTarget> {
    let base = std::env::var(var).ok()?;
    let u = url::Url::parse(&base).ok()?;
    Some(FeTarget {
        host: u.host_str()?.to_string(),
        port: u.port().unwrap_or(9030),
        root_password: None,
        tls_cert_pem: None,
    })
}

struct AllowAllPdp;
#[async_trait::async_trait]
impl egret_core::authz::Pdp for AllowAllPdp {
    async fn authorize(
        &self,
        _s: &egret_core::authz::Subject,
        _a: &str,
        _r: &egret_core::authz::Resource,
    ) -> egret_core::Result<egret_core::authz::Decision> {
        Ok(egret_core::authz::Decision {
            allow: true,
            reasons: vec![],
        })
    }
}

/// A state over the test database with `ddl` as its FE channel.
async fn state(ddl: MysqlDdlRunner) -> EngineMgmtState {
    let dsn = egret_test_db::url()
        .await
        .expect("the live suite needs DATABASE_URL (the setup stores the loader password)");
    let db = egret_metastore::Db::connect(&dsn, 5)
        .await
        .expect("connect");
    db.migrate().await.expect("migrate");
    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secrets = egret_secrets::SecretStore::new(db.clone(), Some(crypto));
    let workspace_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets.clone());
    let databases = egret_query::DatabaseRepo::new(db.clone(), secrets, workspace_secrets.clone());
    EngineMgmtState {
        engine_mgmt: egret_engine_mgmt::EngineMgmtStore::new(db.clone()),
        assignment: egret_engine_mgmt::EngineAssignmentService::new(
            egret_engine_mgmt::EngineMgmtStore::new(db.clone()),
            databases,
        ),
        catalog: Arc::new(egret_catalog_engine::CatalogStore::new(db.clone())),
        storage_registry: egret_storage::StorageRegistry::new(db.clone()),
        authz: Arc::new(egret_policy::PolicyStore::new(db.clone())),
        secrets: workspace_secrets,
        sessions: egret_auth::session::SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        users: egret_auth::users::UserRepo::new(db.clone()),
        roles: egret_auth::roles::RoleRepo::new(db.clone(), egret_events::EventBus::new()),
        pdp: Arc::new(AllowAllPdp),
        engine_provisioner: None,
        kube: None,
        coordinator_transport: None,
        engine_token_minter: None,
        engine_egret_url: "http://egret:8080".into(),
        engine_opa_url: "http://egret:8082".into(),
        engine_jwks_uri: "https://idp/jwks".into(),
        engine_oidc_issuer: "https://idp/realms/egret".into(),
        ranger_enabled: false,
        ranger_surface_mode: egret_engine_mgmt::state::RangerSurfaceMode::Required,
        sr_jdbc_drivers: egret_catalog_registry::JdbcDrivers::default(),
        starrocks_ddl: Arc::new(ddl),
        perm_compiler: None,
        catalog_assignment_compiler: None,
    }
}

async fn seed_engine(st: &EngineMgmtState) -> Engine {
    let spec = serde_json::json!({
        "starrocksConfig": {
            "feImage": "ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.4",
            "cnImage": "ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.4",
            "feReplicas": 1,
            "cnReplicas": 1,
            "delegation": { "mode": "impersonate" },
        },
    });
    st.engine_mgmt
        .create_engine(
            &format!("sr-users-live-{}", Uuid::new_v4().simple()),
            "starrocks",
            "egret-engines",
            &spec,
            None,
        )
        .await
        .expect("create engine row")
}

fn scrub(fe: &FeTarget, e: &FeError) -> String {
    e.scrubbed(fe, &BTreeMap::new())
}

async fn root_query(fe: &FeTarget, sql: &str) -> Vec<Vec<Option<String>>> {
    MysqlDdlRunner
        .query(fe, sql)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {}", scrub(fe, &e)))
}

async fn root_execute(fe: &FeTarget, sql: &str) {
    MysqlDdlRunner
        .execute(fe, sql)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {}", scrub(fe, &e)));
}

fn user_at(name: &str) -> String {
    format!("{}@'%'", quote_string(QueryEngine::StarRocks, name))
}

/// The user names `SHOW USERS` lists (`'<name>'@'<host>'`, read bare).
async fn show_users(fe: &FeTarget) -> BTreeSet<String> {
    root_query(fe, SHOW_USERS_SQL)
        .await
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .map(|raw| {
            raw.strip_prefix('\'')
                .and_then(|r| r.rsplit_once("'@'"))
                .map_or(raw.clone(), |(n, _)| n.to_string())
        })
        .collect()
}

/// The delegator's `SHOW GRANTS` text, every row's `Grants` cell joined.
async fn delegator_grants(fe: &FeTarget) -> String {
    let rows = MysqlDdlRunner
        .query_with_header(fe, &show_delegator_grants_sql())
        .await
        .unwrap_or_else(|e| panic!("grants: {}", scrub(fe, &e)));
    let col = rows[0]
        .iter()
        .position(|c| c.as_deref() == Some("Grants"))
        .expect("a Grants column");
    rows[1..]
        .iter()
        .filter_map(|r| r[col].clone())
        .collect::<Vec<_>>()
        .join("\n")
}

fn opts(fe: &FeTarget, user: &str, password: &str) -> mysql_async::OptsBuilder {
    mysql_async::OptsBuilder::default()
        .ip_or_hostname(fe.host.clone())
        .tcp_port(fe.port)
        .user(Some(user))
        .pass(Some(password))
        .prefer_socket(false)
}

/// Log in as `user`; the login error text when it is refused.
async fn login(fe: &FeTarget, user: &str, password: &str) -> Result<mysql_async::Conn, String> {
    mysql_async::Conn::new(opts(fe, user, password))
        .await
        .map_err(|e| e.to_string())
}

/// The delegator's fresh session runs `sql`; the error text when refused.
async fn delegator_runs(fe: &FeTarget, sql: &str) -> Result<(), String> {
    let mut conn = login(fe, DELEGATOR, DELEGATOR_PASSWORD)
        .await
        .map_err(|e| format!("the delegator could not log in: {e}"))?;
    let out: Result<Vec<mysql_async::Row>, _> = conn.query(sql).await;
    let _ = conn.disconnect().await;
    out.map(|_| ()).map_err(|e| e.to_string())
}

struct Ctx {
    st: EngineMgmtState,
    pool: sqlx::PgPool,
    fe: FeTarget,
    engine: Engine,
    ws: Uuid,
    /// `(user id, FE name)` of the three members: `a`, `b`, and the foreign `c`.
    members: [(Uuid, String); 3],
}

async fn grant_member(pool: &sqlx::PgPool, user: Uuid, ws: Uuid) {
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, id, 'workspace', $2 FROM roles \
          WHERE scope = 'workspace' AND name = 'ws_viewer'",
    )
    .bind(user)
    .bind(ws)
    .execute(pool)
    .await
    .expect("grant membership");
}

fn done(o: UserSync) -> (usize, usize, usize, bool, Vec<String>, Vec<String>) {
    match o {
        UserSync::Done {
            created,
            dropped,
            granted,
            revoked_all,
            foreign,
            failed,
            ..
        } => (created, dropped, granted, revoked_all, foreign, failed),
        other => panic!("expected Done, got {other:?}"),
    }
}

async fn scenario(c: &Ctx) {
    let (fe, st) = (&c.fe, &c.st);
    let [(id_a, a), (_, b), (_, third)] = &c.members;

    // An engine bootstrapped before this package: the delegator may become anyone.
    root_execute(
        fe,
        &format!(
            "CREATE USER {} IDENTIFIED BY '{DELEGATOR_PASSWORD}'",
            user_at(DELEGATOR)
        ),
    )
    .await;
    root_execute(
        fe,
        &format!(
            "GRANT IMPERSONATE ON ALL USERS TO USER {}",
            user_at(DELEGATOR)
        ),
    )
    .await;
    // Someone else's user holding a member's name.
    root_execute(
        fe,
        &format!("CREATE USER {} IDENTIFIED BY 'x'", user_at(third)),
    )
    .await;
    assert!(delegator_grants(fe).await.contains("ON ALL USERS"));

    // First sync.
    let (created, dropped, granted, revoked_all, foreign, failed) =
        done(su::sync_starrocks_users(st, &c.engine, fe).await);
    println!("first sync: created={created} dropped={dropped} granted={granted} revoked_all={revoked_all} foreign={foreign:?} failed={failed:?}");
    assert!(failed.is_empty(), "{failed:?}");
    assert!(created >= 2, "both members created, got {created}");
    assert!(granted >= 2, "both members granted, got {granted}");
    assert!(revoked_all, "ON ALL USERS revoked");
    assert!(foreign.contains(third), "{foreign:?}");
    assert!(!foreign.contains(a) && !foreign.contains(b), "{foreign:?}");

    // Second sync: nothing to do.
    let (created, dropped, granted, revoked_all, _, failed) =
        done(su::sync_starrocks_users(st, &c.engine, fe).await);
    assert!(failed.is_empty(), "{failed:?}");
    assert_eq!(
        (created, dropped, granted, revoked_all),
        (0, 0, 0, false),
        "the second sync is idle"
    );

    // The FE: members exist, nobody can log in as them.
    let users = show_users(fe).await;
    assert!(users.contains(a) && users.contains(b), "{users:?}");
    for who in [a, b] {
        for pw in ["", "x", "anything"] {
            assert!(
                login(fe, who, pw).await.is_err(),
                "{who} logged in with {pw:?}"
            );
        }
    }

    // Native access control.
    let grants = delegator_grants(fe).await;
    println!("delegator grants:\n{grants}");
    assert!(!grants.contains("ON ALL USERS"), "{grants}");
    assert!(
        grants.contains(&user_at(a)) && grants.contains(&user_at(b)),
        "{grants}"
    );
    assert!(!grants.contains(&user_at(third)), "{grants}");
    delegator_runs(fe, &format!("EXECUTE AS `{a}` WITH NO REVERT"))
        .await
        .unwrap_or_else(|e| panic!("impersonating a member: {e}"));
    delegator_runs(fe, &format!("EXECUTE AS `{b}` WITH NO REVERT"))
        .await
        .unwrap_or_else(|e| panic!("impersonating a member: {e}"));
    for target in ["root", third.as_str()] {
        let err = delegator_runs(fe, &format!("EXECUTE AS `{target}` WITH NO REVERT"))
            .await
            .expect_err(&format!("EXECUTE AS {target} must be refused"));
        println!("EXECUTE AS {target}: {err}");
        assert!(err.contains("IMPERSONATE"), "{err}");
    }

    // A member loses access: the next sync drops that user and only that one.
    sqlx::query(
        "DELETE FROM access_grants WHERE principal_type = 'user' AND principal_id = $1 \
            AND scope_type = 'workspace' AND scope_id = $2",
    )
    .bind(id_a)
    .bind(c.ws)
    .execute(&c.pool)
    .await
    .expect("remove the grant");
    let (created, dropped, granted, revoked_all, _, failed) =
        done(su::sync_starrocks_users(st, &c.engine, fe).await);
    assert!(failed.is_empty(), "{failed:?}");
    assert_eq!(
        (created, dropped, granted, revoked_all),
        (0, 1, 0, false),
        "only `a` is dropped"
    );
    let users = show_users(fe).await;
    assert!(!users.contains(a), "{users:?}");
    assert!(users.contains(b) && users.contains(third), "{users:?}");
    let grants = delegator_grants(fe).await;
    assert!(!grants.contains(&user_at(a)), "{grants}");
    assert!(grants.contains(&user_at(b)), "{grants}");
}

/// Everything the test made, on the FE and in the database; best effort.
async fn cleanup(c: &Ctx) {
    let recorded: Vec<String> =
        sqlx::query_scalar("SELECT username FROM engine_mgmt.starrocks_users WHERE engine_id = $1")
            .bind(c.engine.id)
            .fetch_all(&c.pool)
            .await
            .unwrap_or_default();
    let mut names: BTreeSet<String> = recorded.into_iter().collect();
    names.extend(c.members.iter().map(|(_, n)| n.clone()));
    for n in names {
        let _ = MysqlDdlRunner
            .execute(&c.fe, &format!("DROP USER IF EXISTS {}", user_at(&n)))
            .await;
    }
    let _ = MysqlDdlRunner
        .execute(
            &c.fe,
            &format!("DROP USER IF EXISTS {}", user_at(DELEGATOR)),
        )
        .await;
    let pool = &c.pool;
    let ids: Vec<Uuid> = c.members.iter().map(|(i, _)| *i).collect();
    for sql in [
        "DELETE FROM access_grants WHERE scope_type = 'workspace' AND scope_id = $1",
        "DELETE FROM engine_mgmt.starrocks_users WHERE engine_id = $1",
    ] {
        let target = if sql.contains("starrocks_users") {
            c.engine.id
        } else {
            c.ws
        };
        let _ = sqlx::query(sql).bind(target).execute(pool).await;
    }
    let _ = c.st.engine_mgmt.delete_engine(c.engine.id).await;
    let _ = sqlx::query("DELETE FROM workspaces WHERE id = $1")
        .bind(c.ws)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE id = ANY($1)")
        .bind(&ids)
        .execute(pool)
        .await;
}

#[tokio::test]
async fn the_user_sync_against_a_real_fe() {
    let Some(fe) = fe_from("EGRET_TEST_STARROCKS_URL") else {
        eprintln!("EGRET_TEST_STARROCKS_URL unset: skipping the StarRocks user sync live suite");
        return;
    };
    let started = Instant::now();
    let st = state(MysqlDdlRunner).await;
    let dsn = egret_test_db::url().await.expect("DATABASE_URL");
    let pool = egret_metastore::Db::connect(&dsn, 5)
        .await
        .expect("connect")
        .pool;
    let engine = seed_engine(&st).await;
    let suffix = Uuid::new_v4().simple().to_string()[..12].to_string();
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, $1) RETURNING id")
            .bind(format!("pkgd-live-{suffix}"))
            .fetch_one(&pool)
            .await
            .expect("workspace");
    let mut members = Vec::new();
    for tag in ["a", "b", "c"] {
        let name = format!("pkgd_live_{suffix}_{tag}");
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO users (provider, email, username) VALUES ('local', $1, $2) RETURNING id",
        )
        .bind(format!("{name}@live.test"))
        .bind(&name)
        .fetch_one(&pool)
        .await
        .expect("user");
        grant_member(&pool, id, ws).await;
        members.push((id, name));
    }
    st.engine_mgmt
        .assign_workspace(engine.id, ws, members[0].0)
        .await
        .expect("assign the workspace");
    let ctx = Arc::new(Ctx {
        st,
        pool,
        fe,
        engine,
        ws,
        members: members.try_into().unwrap(),
    });
    let run = {
        let ctx = Arc::clone(&ctx);
        tokio::spawn(async move { scenario(&ctx).await })
    };
    let result = run.await;
    cleanup(&ctx).await;
    println!("live suite took {:?}", started.elapsed());
    if let Err(e) = result {
        std::panic::resume_unwind(e.into_panic());
    }
}
