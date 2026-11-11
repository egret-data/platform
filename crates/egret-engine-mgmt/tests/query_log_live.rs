//! Engine-gated: the Query log's setup and its list read against real
//! StarRocks FEs (package A, Task 5). Self-skips unless
//! `EGRET_TEST_STARROCKS_URL` (egret's `4.1.7-egret.4` image, which ships the
//! AuditLoader) or `EGRET_TEST_STARROCKS_OFFICIAL_URL` (the official
//! `starrocks/allin1-ubuntu:4.1.7`, which does not) is set, so it reports `ok`
//! while asserting nothing: the DURATION is what tells a pass from a skip. The
//! egret.4 test takes minutes (a plugin refresh and two batch intervals).
//!
//! Both need the test database too (`DATABASE_URL`, through `egret-test-db`):
//! the setup mints and stores the loader password and checks the loader's
//! username like production does. The FE target is built here, as `root` with
//! no password (the allin1 containers' root has none); the engine row carries
//! only the FE image, which decides whether the setup installs the plugin.
//!
//! The setup and the read are the crate's own functions, reached through the
//! `test-util` entry point `egret_engine_mgmt::query_log_live_testing`.
//!
//! Run with (the two containers already up):
//!   env -u EGRET_ALLOW_DB_SKIP \
//!     DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!     EGRET_TEST_STARROCKS_URL=mysql://127.0.0.1:19030 \
//!     EGRET_TEST_STARROCKS_OFFICIAL_URL=mysql://127.0.0.1:29030 \
//!     cargo test -p egret-engine-mgmt --test query_log_live -- --nocapture

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egret_engine_mgmt::query_log_live_testing::{
    self as ql, QueryLogEntryDto, QueryLogParams, QueryLogSetup, READ_MARKER,
};
use egret_engine_mgmt::{Engine, EngineMgmtState, FeError, FeTarget, MysqlDdlRunner, StarRocksDdl};
use egret_query_api::{quote_string, Engine as QueryEngine};
use mysql_async::prelude::Queryable;
use uuid::Uuid;

/// An image `egret_perm::is_patched_starrocks_fe` accepts: the setup installs.
const EGRET_4_FE_IMAGE: &str = "ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.4";
/// The official image the second container runs: the setup does not install.
const OFFICIAL_IMAGE: &str = "starrocks/allin1-ubuntu:4.1.7";

/// The second user (runs a statement as itself) and the third (runs one as
/// the second through `EXECUTE AS`). Made by this suite, on its own FE.
const BOB: &str = "egret_t5_bob";
const CAROL: &str = "egret_t5_carol";

/// One plugin refresh, two batch intervals and margin.
const EVENT_BUDGET: Duration = Duration::from_secs(240);
const POLL: Duration = Duration::from_secs(5);

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

/// The real runner, recording every statement it executes, so a test can
/// say what the setup sent.
#[derive(Clone, Default)]
struct Recording {
    executed: Arc<Mutex<Vec<String>>>,
}

impl Recording {
    fn executed(&self) -> Vec<String> {
        self.executed.lock().unwrap().clone()
    }
    fn clear(&self) {
        self.executed.lock().unwrap().clear();
    }
    fn installs(&self) -> usize {
        self.executed()
            .iter()
            .filter(|s| s.starts_with("INSTALL PLUGIN"))
            .count()
    }
}

#[async_trait::async_trait]
impl StarRocksDdl for Recording {
    async fn execute(&self, fe: &FeTarget, sql: &str) -> Result<(), FeError> {
        self.executed.lock().unwrap().push(sql.to_string());
        MysqlDdlRunner.execute(fe, sql).await
    }
    async fn query(&self, fe: &FeTarget, sql: &str) -> Result<Vec<Vec<Option<String>>>, FeError> {
        MysqlDdlRunner.query(fe, sql).await
    }
    async fn query_with_header(
        &self,
        fe: &FeTarget,
        sql: &str,
    ) -> Result<Vec<Vec<Option<String>>>, FeError> {
        MysqlDdlRunner.query_with_header(fe, sql).await
    }
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
async fn state(ddl: Recording) -> EngineMgmtState {
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

/// A StarRocks engine row whose stored FE image is `fe_image`, bound to no
/// policy profile (so the setup sends the native grant).
async fn seed_engine(st: &EngineMgmtState, fe_image: &str) -> Engine {
    let spec = serde_json::json!({
        "imageVersion": fe_image,
        "starrocksConfig": {
            "feImage": fe_image,
            "cnImage": fe_image,
            "feReplicas": 1,
            "cnReplicas": 1,
            "feResources": null,
            "cnResources": null,
            "delegation": null,
        },
    });
    st.engine_mgmt
        .create_engine(
            &format!("sr-live-{}", Uuid::new_v4().simple()),
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

fn first_cell(rows: &[Vec<Option<String>>]) -> Option<String> {
    rows.first().and_then(|r| r.first()).cloned().flatten()
}

fn user_at(name: &str) -> String {
    format!("{}@'%'", quote_string(QueryEngine::StarRocks, name))
}

/// A user with `password`, created or converged.
async fn ensure_user(fe: &FeTarget, name: &str, password: &str) {
    let pw = quote_string(QueryEngine::StarRocks, password);
    let who = user_at(name);
    root_execute(
        fe,
        &format!("CREATE USER IF NOT EXISTS {who} IDENTIFIED BY {pw}"),
    )
    .await;
    root_execute(fe, &format!("ALTER USER {who} IDENTIFIED BY {pw}")).await;
}

/// A session of `user` on `fe`, its statements run one after another on the
/// one connection (so `EXECUTE AS` holds for the next).
async fn run_as(fe: &FeTarget, user: &str, password: &str, statements: &[String]) {
    let opts = mysql_async::OptsBuilder::default()
        .ip_or_hostname(fe.host.clone())
        .tcp_port(fe.port)
        .user(Some(user))
        .pass(Some(password))
        .prefer_socket(false);
    let mut conn = mysql_async::Conn::new(opts)
        .await
        .unwrap_or_else(|e| panic!("{user} logs in: {e}"));
    for s in statements {
        let _: Vec<mysql_async::Row> = conn
            .query(s.as_str())
            .await
            .unwrap_or_else(|e| panic!("{user} runs {s}: {e}"));
    }
    let _ = conn.disconnect().await;
}

fn rfc3339(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Every entry of the log from `from` to now (plus a minute), read page by
/// page through the list route's own read, following `nextCursor` with pages
/// of `limit`. Returns the entries and the number of pages.
async fn read_all(
    st: &EngineMgmtState,
    fe: &FeTarget,
    from: chrono::DateTime<chrono::Utc>,
    user: Option<&str>,
    limit: u32,
) -> (Vec<QueryLogEntryDto>, usize) {
    let to = chrono::Utc::now() + chrono::Duration::minutes(1);
    let mut entries = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let params = QueryLogParams {
            from: Some(rfc3339(from)),
            to: Some(rfc3339(to)),
            user: user.map(str::to_string),
            state: None,
            limit: Some(limit.to_string()),
            cursor: cursor.take(),
        };
        let page = ql::list_page(st, fe, &params)
            .await
            .unwrap_or_else(|e| panic!("the list read failed on the FE: {e}"));
        pages += 1;
        assert!(
            page.entries.len() <= limit as usize,
            "a page is at most limit"
        );
        entries.extend(page.entries);
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
        assert!(pages < 500, "paging does not end");
    }
    (entries, pages)
}

fn with_marker<'a>(entries: &'a [QueryLogEntryDto], marker: &str) -> Option<&'a QueryLogEntryDto> {
    entries.iter().find(|e| {
        e.statement.as_deref().is_some_and(|s| s.contains(marker))
            && e.statement
                .as_deref()
                .is_some_and(|s| s.starts_with("SELECT"))
    })
}

/// Poll `check` every [`POLL`] until it answers `Some`, within [`EVENT_BUDGET`].
async fn eventually<T, F, Fut>(what: &str, mut check: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let start = Instant::now();
    loop {
        if let Some(v) = check().await {
            eprintln!("{what}: after {:?}", start.elapsed());
            return v;
        }
        assert!(
            start.elapsed() < EVENT_BUDGET,
            "{what}: not within {EVENT_BUDGET:?}"
        );
        tokio::time::sleep(POLL).await;
    }
}

/// egret.4: the setup installs the plugin and is `Ready`, events then arrive;
/// a second setup installs nothing; a statement of a second user and one run
/// `EXECUTE AS` that user by a third come back through the list read with the
/// right `user` and `authorizedUser`, filtered by either name and paged by
/// cursor; egret's own reads are in the table but never in the list.
#[tokio::test]
async fn egret_4_sets_up_the_query_log_and_reads_it_back() {
    let Some(fe) = fe_from("EGRET_TEST_STARROCKS_URL") else {
        eprintln!("skip: EGRET_TEST_STARROCKS_URL not set");
        return;
    };
    let started = Instant::now();
    let since = chrono::Utc::now() - chrono::Duration::minutes(1);
    let ddl = Recording::default();
    let st = state(ddl.clone()).await;
    let engine = seed_engine(&st, EGRET_4_FE_IMAGE).await;

    // Premise: no AuditLoader, so the first setup must install it. An FE
    // without one answers UNINSTALL with an error, which changes nothing.
    let _ = MysqlDdlRunner
        .execute(&fe, "UNINSTALL PLUGIN AuditLoader")
        .await;
    let plugins = MysqlDdlRunner
        .query_with_header(&fe, "SHOW PLUGINS")
        .await
        .unwrap_or_else(|e| panic!("SHOW PLUGINS: {}", scrub(&fe, &e)));
    assert!(
        !plugins
            .iter()
            .skip(1)
            .any(|r| r.first().cloned().flatten().as_deref() == Some("AuditLoader")),
        "premise: AuditLoader uninstalled: {plugins:?}"
    );

    // 1. The first setup installs, waits for the probe, and is Ready.
    let setup = ql::ensure_query_log(&st, &engine, &fe, true).await;
    assert_eq!(setup, QueryLogSetup::Ready, "{:?}", ddl.executed());
    assert_eq!(ddl.installs(), 1, "{:?}", ddl.executed());
    assert!(
        ddl.executed()
            .iter()
            .any(|s| s.starts_with("CREATE TABLE IF NOT EXISTS") && s.ends_with(");")),
        "premise: the real runner was sent the table DDL with its trailing `;`"
    );
    // The probe is marked as egret's own read (final review, M1): it is in
    // the table, under the marker, and step 4 checks it is not in the list.
    let probes = eventually("the probe row", || async {
        let n = first_cell(
            &root_query(
                &fe,
                &format!(
                    "SELECT count(*) FROM starrocks_audit_db__.starrocks_audit_tbl__ \
                     WHERE stmt LIKE concat('{READ_MARKER} SELECT ''egret-query-log-', \
                     'probe-%') \
                     AND timestamp >= '{}'",
                    since.format("%Y-%m-%d %H:%M:%S")
                ),
            )
            .await,
        )?;
        (n != "0").then_some(n)
    })
    .await;
    eprintln!("probe rows: {probes}");
    eprintln!("setup with events after {:?}", started.elapsed());

    // 2. A second setup installs nothing (and is still Ready).
    ddl.clear();
    let engine = st
        .engine_mgmt
        .get_engine(engine.id)
        .await
        .expect("reload")
        .expect("engine present");
    let again = ql::ensure_query_log(&st, &engine, &fe, false).await;
    assert_eq!(again, QueryLogSetup::Ready);
    assert_eq!(ddl.installs(), 0, "{:?}", ddl.executed());
    assert!(!ddl.executed().is_empty(), "premise: the steps ran again");

    // The FE's answer to a second INSTALL is read as "already installed".
    let e = MysqlDdlRunner
        .execute(
            &fe,
            "INSTALL PLUGIN FROM \"/opt/egret/auditloader.zip\" PROPERTIES (\"user\" = 'x')",
        )
        .await
        .expect_err("a second INSTALL is refused");
    assert!(e.is_already_installed(), "{}", scrub(&fe, &e));

    // 3. Bob runs a statement; Carol runs one as Bob.
    let bob_pw = format!("Bp-{}", Uuid::new_v4().simple());
    let carol_pw = format!("Cp-{}", Uuid::new_v4().simple());
    ensure_user(&fe, BOB, &bob_pw).await;
    ensure_user(&fe, CAROL, &carol_pw).await;
    root_execute(
        &fe,
        &format!(
            "GRANT IMPERSONATE ON USER {} TO USER {}",
            user_at(BOB),
            user_at(CAROL)
        ),
    )
    .await;
    let run = Uuid::new_v4().simple().to_string();
    let direct = format!("egret-t5-direct-{run}");
    let as_bob = format!("egret-t5-as-bob-{run}");
    let select = |m: &str| format!("SELECT {}", quote_string(QueryEngine::StarRocks, m));
    run_as(&fe, BOB, &bob_pw, &[select(&direct)]).await;
    run_as(
        &fe,
        CAROL,
        &carol_pw,
        &[
            format!("EXECUTE AS {} WITH NO REVERT", user_at(BOB)),
            select(&as_bob),
        ],
    )
    .await;

    // Filtered by Bob: his own row by `user`, Carol's by the name inside
    // `authorizedUser` (the quoted-name regex, on the FE).
    let by_bob = eventually("both rows, filtered by bob", || async {
        let (entries, _) = read_all(&st, &fe, since, Some(BOB), 50).await;
        (with_marker(&entries, &direct).is_some() && with_marker(&entries, &as_bob).is_some())
            .then_some(entries)
    })
    .await;
    let mine = with_marker(&by_bob, &direct).unwrap();
    assert_eq!(mine.user.as_deref(), Some(BOB), "{mine:?}");
    assert_eq!(mine.authorized_user.as_deref(), Some(BOB), "{mine:?}");
    let delegated = with_marker(&by_bob, &as_bob).unwrap();
    assert_eq!(delegated.user.as_deref(), Some(CAROL), "{delegated:?}");
    assert_eq!(
        delegated.authorized_user.as_deref(),
        Some(BOB),
        "{delegated:?}"
    );
    for e in &by_bob {
        assert!(
            e.user.as_deref() == Some(BOB) || e.authorized_user.as_deref() == Some(BOB),
            "the bob filter let another user's row through: {e:?}"
        );
    }
    // Filtered by Carol (uppercase: the filter is case-insensitive): her row,
    // not Bob's own.
    let (by_carol, _) = read_all(&st, &fe, since, Some("EGRET_T5_CAROL"), 50).await;
    assert!(with_marker(&by_carol, &as_bob).is_some(), "{by_carol:?}");
    assert!(with_marker(&by_carol, &direct).is_none(), "{by_carol:?}");

    // 4. egret's own reads: in the table (the marker comment is kept)...
    let own = eventually("an egret read in the table", || async {
        let n = first_cell(
            &root_query(
                &fe,
                &format!(
                    "SELECT count(*) FROM starrocks_audit_db__.starrocks_audit_tbl__ \
                     WHERE `user` = 'root' AND stmt LIKE '{READ_MARKER}%' \
                     AND timestamp >= '{}'",
                    since.format("%Y-%m-%d %H:%M:%S")
                ),
            )
            .await,
        )?;
        (n != "0").then_some(n)
    })
    .await;
    eprintln!("egret reads in the table since the start: {own}");
    // ...and never in the list, read whole in small pages through the cursor.
    let (all, pages) = read_all(&st, &fe, since, None, 3).await;
    eprintln!("unfiltered: {} entries in {pages} pages", all.len());
    assert!(pages > 1, "premise: the cursor was followed ({pages} page)");
    assert!(with_marker(&all, &direct).is_some(), "paging lost a row");
    assert!(with_marker(&all, &as_bob).is_some(), "paging lost a row");
    let mut ids: Vec<_> = all.iter().map(|e| (&e.timestamp, &e.query_id)).collect();
    let before = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), before, "a row came twice across pages");
    for e in &all {
        assert!(
            !e.statement
                .as_deref()
                .is_some_and(|s| s.starts_with(READ_MARKER)),
            "egret's own read in the list: {e:?}"
        );
        assert!(
            !e.statement
                .as_deref()
                .is_some_and(|s| s.contains("egret-query-log-probe-")),
            "egret's probe or its poll in the list: {e:?}"
        );
    }

    // The detail and newest-event SQL run on this FE too.
    let id = Uuid::parse_str(&delegated.query_id).expect("a queryId is a UUID");
    let detail = root_query(&fe, &ql::detail_sql(&id)).await;
    assert_eq!(
        first_cell(&detail).as_deref(),
        Some(id.to_string().as_str())
    );
    let newest = root_query(&fe, &ql::last_event_sql(chrono::Utc::now())).await;
    assert!(first_cell(&newest).is_some(), "{newest:?}");

    eprintln!("egret.4 live test took {:?}", started.elapsed());
}

/// The FE errors the setup and the reads branch on, as the real runner
/// classifies them (measured texts in `task-5-measurements.md`).
#[tokio::test]
async fn egret_4_errors_are_classified_from_the_real_fe() {
    let Some(fe) = fe_from("EGRET_TEST_STARROCKS_URL") else {
        eprintln!("skip: EGRET_TEST_STARROCKS_URL not set");
        return;
    };
    let started = Instant::now();
    let e = MysqlDdlRunner
        .query(&fe, "SHOW TABLES FROM nodb__")
        .await
        .expect_err("no such database");
    assert!(e.is_unknown_relation(), "{}", scrub(&fe, &e));
    let e = MysqlDdlRunner
        .query(&fe, "SELECT 1 FROM starrocks_audit_db__.nope")
        .await
        .expect_err("no such table");
    assert!(e.is_unknown_relation(), "{}", scrub(&fe, &e));
    let e = MysqlDdlRunner
        .execute(&fe, "INSTALL PLUGIN FROM \"/nonexistent.zip\"")
        .await
        .expect_err("no such zip");
    assert!(!e.is_already_installed(), "{}", scrub(&fe, &e));
    assert!(!e.is_unknown_relation(), "{}", scrub(&fe, &e));
    let text = scrub(&fe, &e);
    assert!(text.contains("zip file does not exist"), "{text}");
    eprintln!("error classification took {:?}", started.elapsed());
}

/// The official image: no plugin, so `PluginNotShipped`, and no INSTALL is
/// sent; the database, the table (created fresh, through the real runner,
/// trailing `;` included) and the loader user still exist.
#[tokio::test]
async fn the_official_image_gets_the_table_and_user_but_no_plugin() {
    let Some(fe) = fe_from("EGRET_TEST_STARROCKS_OFFICIAL_URL") else {
        eprintln!("skip: EGRET_TEST_STARROCKS_OFFICIAL_URL not set");
        return;
    };
    let started = Instant::now();
    // Premise: nothing of the query log on this FE yet.
    root_execute(&fe, "DROP DATABASE IF EXISTS starrocks_audit_db__ FORCE").await;
    root_execute(&fe, "DROP USER IF EXISTS 'egret_query_log'@'%'").await;
    let ddl = Recording::default();
    let st = state(ddl.clone()).await;
    let engine = seed_engine(&st, OFFICIAL_IMAGE).await;

    let setup = ql::ensure_query_log(&st, &engine, &fe, true).await;

    assert_eq!(
        setup,
        QueryLogSetup::PluginNotShipped,
        "{:?}",
        ddl.executed()
    );
    assert_eq!(ddl.installs(), 0, "{:?}", ddl.executed());
    let tables = root_query(&fe, "SHOW TABLES FROM starrocks_audit_db__").await;
    assert!(
        tables
            .iter()
            .any(|r| r.first().cloned().flatten().as_deref() == Some("starrocks_audit_tbl__")),
        "{tables:?}"
    );
    let users = root_query(&fe, "SHOW USERS").await;
    assert!(
        users
            .iter()
            .any(|r| r.first().cloned().flatten().as_deref() == Some("'egret_query_log'@'%'")),
        "{users:?}"
    );
    let plugins = MysqlDdlRunner
        .query_with_header(&fe, "SHOW PLUGINS")
        .await
        .unwrap_or_else(|e| panic!("SHOW PLUGINS: {}", scrub(&fe, &e)));
    assert!(
        !plugins
            .iter()
            .any(|r| r.iter().any(|v| v.as_deref() == Some("AuditLoader"))),
        "{plugins:?}"
    );
    eprintln!("official live test took {:?}", started.elapsed());
}
