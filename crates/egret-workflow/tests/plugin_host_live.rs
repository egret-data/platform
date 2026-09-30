//! **W5b Task 6: Kestra plugin tasks through the real worker, the real Docker
//! runner and the real plugin host image** (`egret-plugin-host:dev`).
//!
//! Gated: every test is `#[ignore]`, and runs with
//!
//! ```bash
//! EGRET_PLUGIN_HOST_LIVE=1 DATABASE_URL=… \
//!   cargo test -p egret-workflow --test plugin_host_live -- --ignored --test-threads=1
//! ```
//!
//! Run with `--ignored` but without `EGRET_PLUGIN_HOST_LIVE=1`, a test FAILS
//! (it does not pass having asserted nothing). The Trino and StarRocks legs
//! print `SKIPPED: <reason>` when their engine is unavailable; the Postgres
//! leg is never skipped.
//!
//! Each test is a `#[sqlx::test]`: a scratch database, so the runtime's
//! global claim sees only this test's runs, and **the closing scan covers
//! every row of every `workflow.*` table in that database**.
//!
//! The engines are reached from the plugin container at
//! `host.docker.internal:<published port>` (Docker Desktop). Images are the
//! Task 1 pins (`oracle-exec/docker-compose.w5b.yml`).

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egret_metastore::Db;
use egret_secrets::{Crypto, SecretStore, WorkspaceSecretStore};
use egret_workflow::logs::LogLimits;
use egret_workflow::runners::secrets::{SecretError, SecretSource};
use egret_workflow::runtime::{spawn_with_logs, RuntimeConfig};
use egret_workflow::scripts::RunnerKind;
use egret_workflow::worker::commands::DockerRunner;
use egret_workflow::worker::handlers::kestra::{register, KestraRuntime};
use egret_workflow::worker::handlers::script::PgFeatureState;
use egret_workflow::worker::{start_execution, Handlers};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const GATE: &str = "EGRET_PLUGIN_HOST_LIVE";
const HOST_IMAGE: &str = "egret-plugin-host:dev";
const PG_IMAGE: &str =
    "postgres:18@sha256:4ef4dbc939d61acea57712655ddb4b4ab27419c913f94cca0cd57cb3ea3c2280";
const TRINO_IMAGE: &str =
    "trinodb/trino@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534";
const STARROCKS_IMAGE: &str =
    "starrocks/allin1-ubuntu@sha256:faf7ce9c24d9c29c9431b4e8cbd4bb7a74cd169907c63f0c5ebaacc7f9df276b";

/// The Postgres target's password: the workspace secret's value. Written
/// nowhere in this file's flows -- they say `secret('pg', …)`.
const PG_PASSWORD: &str = "w5bT6-Pw-9f3c1e";

fn gate() {
    assert_eq!(
        std::env::var(GATE).as_deref(),
        Ok("1"),
        "set {GATE}=1 to run the plugin host live tests (Docker, {HOST_IMAGE}, {PG_IMAGE})"
    );
}

async fn docker(args: &[&str]) -> (bool, String) {
    let out = Command::new("docker")
        .args(args)
        .output()
        .await
        .expect("docker on PATH");
    let mut text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() {
        text.push_str(&String::from_utf8_lossy(&out.stderr));
    }
    (out.status.success(), text)
}

/// A container removed on drop.
struct Engine {
    name: String,
    port: u16,
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "--force", &self.name])
            .output();
    }
}

/// `docker run -d -P` and the host port published for `inner`.
async fn start_engine(image: &str, inner: u16, extra: &[&str]) -> Result<Engine, String> {
    let name = format!("egret-w5b-t6-{}", Uuid::new_v4().simple());
    let port_spec = format!("127.0.0.1::{inner}");
    let mut args = vec!["run", "-d", "--name", &name, "-p", &port_spec];
    args.extend_from_slice(extra);
    args.push(image);
    let (ok, out) = docker(&args).await;
    if !ok {
        return Err(format!("docker run {image}: {out}"));
    }
    let mut engine = Engine { name, port: 0 };
    let (ok, out) = docker(&["port", &engine.name, &inner.to_string()]).await;
    if !ok {
        return Err(format!("docker port: {out}"));
    }
    engine.port = out
        .lines()
        .next()
        .and_then(|l| l.rsplit(':').next())
        .and_then(|p| p.parse().ok())
        .ok_or_else(|| format!("no published port in {out:?}"))?;
    Ok(engine)
}

async fn start_postgres() -> Engine {
    gate();
    let pw = format!("POSTGRES_PASSWORD={PG_PASSWORD}");
    let pg = start_engine(
        PG_IMAGE,
        5432,
        &[
            "-e",
            "POSTGRES_USER=w5b",
            "-e",
            "POSTGRES_DB=w5b",
            "-e",
            &pw,
        ],
    )
    .await
    .expect("the Postgres target starts");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let (ok, _) = docker(&[
            "exec", &pg.name, "psql", "-U", "w5b", "-d", "w5b", "-tAc", "SELECT 1",
        ])
        .await;
        if ok {
            // The image's init restarts the server once; give it a moment.
            tokio::time::sleep(Duration::from_secs(2)).await;
            let (ok, _) = docker(&[
                "exec", &pg.name, "psql", "-U", "w5b", "-d", "w5b", "-tAc", "SELECT 1",
            ])
            .await;
            if ok {
                return pg;
            }
        }
        assert!(Instant::now() < deadline, "Postgres did not come up");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn psql(pg: &Engine, sql: &str) -> String {
    let (ok, out) = docker(&[
        "exec", &pg.name, "psql", "-U", "w5b", "-d", "w5b", "-tAc", sql,
    ])
    .await;
    assert!(ok, "psql {sql}: {out}");
    out
}

/// `secret()` over the real `workspace_secrets`, as egret-bin's
/// `WorkspaceSecretSource` (this crate cannot depend on egret-bin).
struct StoreSource(WorkspaceSecretStore);

#[async_trait::async_trait]
impl SecretSource for StoreSource {
    async fn get(&self, ws: Uuid, name: &str) -> Result<BTreeMap<String, String>, SecretError> {
        match self.0.resolve_typed_by_name(ws, name).await {
            Ok(t) => Ok(t.data),
            Err(egret_secrets::SecretsError::NotFound) => {
                Err(SecretError::NotFound { name: name.into() })
            }
            Err(e) => Err(SecretError::Unavailable {
                name: name.into(),
                reason: e.to_string(),
            }),
        }
    }
}

struct Fx {
    db: Db,
    ws: Uuid,
    user: egret_core::UserId,
    store: WorkspaceSecretStore,
}

async fn fx(pool: &sqlx::PgPool) -> Fx {
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::enable_feature(&db, ws, "workflow_containers").await;
    common::enable_feature(&db, ws, "workflow_kestra_plugins").await;
    common::grant(&db, ws, user, &["workflow:read"]).await;
    let crypto = Crypto::new(b"0123456789abcdef0123456789abcdef").unwrap();
    let store = WorkspaceSecretStore::new(db.clone(), SecretStore::new(db.clone(), Some(crypto)));
    Fx {
        db,
        ws,
        user,
        store,
    }
}

impl Fx {
    async fn secret(&self, name: &str, pairs: &[(&str, &str)]) {
        let data: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        self.store
            .create_typed(Some(self.ws), name, None, "kubernetes.io/basic-auth", &data)
            .await
            .expect("create secret");
    }

    /// The production registry's shape: core tasks + the plugin handler over
    /// a real `DockerRunner`, the real feature state, no `plugin_configurations`
    /// (no `kestra` entry allows the whole catalog; no `allowed-images`
    /// anywhere, which would refuse every script image).
    fn handlers(&self, kill_grace_s: u64) -> Handlers {
        let mut h = Handlers::with_core_tasks();
        register(
            &mut h,
            Arc::new(KestraRuntime {
                runner: Arc::new(
                    DockerRunner::new()
                        .with_logs(egret_workflow::logs::LogRepo::new(self.db.pool.clone())),
                ),
                runner_kind: RunnerKind::Docker,
                features: Arc::new(PgFeatureState::new(self.db.pool.clone())),
                plugin_configurations: Default::default(),
                host: egret_config::PluginHostConfig {
                    image: Some(HOST_IMAGE.into()),
                    kill_grace_s,
                    ..Default::default()
                },
                secrets: Arc::new(StoreSource(self.store.clone())),
            }),
        );
        h
    }

    /// Run `source` to a terminal state through `spawn_with_logs`, then stop
    /// the runtime (its join drains the log writer).
    async fn run(&self, source: &str, kill_grace_s: u64, within: Duration) -> Uuid {
        let exec = start_execution(&self.db.pool, self.ws, None, source, serde_json::json!({}))
            .await
            .expect("start execution");
        let cancel = CancellationToken::new();
        let rt = spawn_with_logs(
            self.db.pool.clone(),
            self.handlers(kill_grace_s),
            RuntimeConfig {
                enabled: true,
                tick_interval: Duration::from_millis(100),
                batch: 4,
                reaper_interval: Duration::from_secs(3600),
                working_timeout: Duration::from_secs(3600),
                container_default_timeout: egret_workflow::worker::DEFAULT_CONTAINER_TIMEOUT,
                scheduler_enabled: false,
                trigger_interval: Duration::from_secs(3600),
                log_limits: LogLimits::default(),
                log_retention_days: 0,
            },
            cancel.clone(),
        );
        let start = Instant::now();
        loop {
            let state: String =
                sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
                    .bind(exec)
                    .fetch_one(&self.db.pool)
                    .await
                    .unwrap();
            if ["SUCCESS", "FAILED", "WARNING", "KILLED"].contains(&state.as_str()) {
                break;
            }
            assert!(
                start.elapsed() < within,
                "execution still {state} after {within:?}"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(60), rt.join())
            .await
            .expect("the runtime stops");
        exec
    }

    async fn task_run(
        &self,
        exec: Uuid,
        task: &str,
    ) -> (String, Option<serde_json::Value>, Option<String>) {
        sqlx::query_as(
            "SELECT state, outputs, message FROM workflow.task_runs \
              WHERE execution_id = $1 AND task_id = $2 ORDER BY attempt DESC LIMIT 1",
        )
        .bind(exec)
        .bind(task)
        .fetch_one(&self.db.pool)
        .await
        .expect("the task run")
    }

    /// The lines of `exec`, through W11's read API as a `workflow:read`
    /// member. Waits out the 2 s settle lag.
    async fn api_lines(&self, exec: Uuid) -> Vec<(String, String)> {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let app = common::app(&self.db).await;
        let res = common::get(
            &app,
            &format!(
                "/api/ws/{}/workflows/executions/{exec}/logs?limit=2000",
                self.ws
            ),
            self.user,
        )
        .await;
        assert_eq!(res.status().as_u16(), 200);
        let body: serde_json::Value =
            serde_json::from_str(&common::body_text(res).await).expect("json");
        body["items"]
            .as_array()
            .unwrap_or_else(|| panic!("no items in {body}"))
            .iter()
            .map(|i| {
                (
                    i["level"].as_str().unwrap_or_default().to_string(),
                    i["message"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect()
    }
}

/// **Every column of every `workflow.*` table, as text, for `needle`.**
/// Returns `(rows scanned, cells scanned, matches)`.
async fn scan_workflow_schema(pool: &sqlx::PgPool, needle: &str) -> (i64, i64, Vec<String>) {
    let cols: Vec<(String, String)> = sqlx::query_as(
        "SELECT c.table_name::text, c.column_name::text FROM information_schema.columns c \
           JOIN information_schema.tables t \
             ON t.table_schema = c.table_schema AND t.table_name = c.table_name \
          WHERE c.table_schema = 'workflow' AND t.table_type = 'BASE TABLE' \
          ORDER BY 1, 2",
    )
    .fetch_all(pool)
    .await
    .expect("columns");
    assert!(!cols.is_empty(), "the workflow schema has columns");
    let mut rows = 0i64;
    let mut cells = 0i64;
    let mut hits = Vec::new();
    let mut tables: Vec<&str> = cols.iter().map(|(t, _)| t.as_str()).collect();
    tables.dedup();
    for t in &tables {
        let n: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM workflow.\"{t}\""
        )))
        .fetch_one(pool)
        .await
        .expect("count");
        rows += n;
    }
    for (t, c) in &cols {
        let (n, hit): (i64, i64) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT count(\"{c}\"), count(*) FILTER (WHERE strpos(\"{c}\"::text, $1) > 0) \
               FROM workflow.\"{t}\""
        )))
        .bind(needle)
        .fetch_one(pool)
        .await
        .unwrap_or_else(|e| panic!("scan {t}.{c}: {e}"));
        cells += n;
        if hit > 0 {
            hits.push(format!("{t}.{c}: {hit}"));
        }
    }
    (rows, cells, hits)
}

/// The scan, asserted, and printed beside what it examined.
async fn assert_no_secret_stored(pool: &sqlx::PgPool, needle: &str) {
    let (rows, cells, hits) = scan_workflow_schema(pool, needle).await;
    println!(
        "DB SCAN: {} matches for the secret in {rows} rows / {cells} non-null cells of every \
         workflow.* table",
        hits.len()
    );
    assert!(rows > 0, "the scan must examine rows to mean anything");
    assert!(hits.is_empty(), "the secret's value is stored: {hits:?}");
}

fn pg_flow(id: &str, port: u16, sql: &str, extra: &str) -> String {
    format!(
        "id: {id}\nnamespace: w5b\ntasks:\n  - id: q\n    type: kestra.io.kestra.plugin.jdbc.postgresql.Query\n    \
         url: jdbc:postgresql://host.docker.internal:{port}/w5b\n    \
         username: \"{{{{ secret('pg', subkey='username') }}}}\"\n    \
         password: \"{{{{ secret('pg', subkey='password') }}}}\"\n    \
         sql: \"{sql}\"\n{extra}"
    )
}

/// **A `kestra.…postgresql.Query` runs end to end**: rows in `outputs`, its
/// lines through W11's API, the password from a workspace secret, a line that
/// echoes the secret stored as `******`; then Trino and StarRocks (skippable);
/// then the scan of every `workflow.*` row.
#[sqlx::test(migrations = "../../migrations")]
#[ignore = "needs EGRET_PLUGIN_HOST_LIVE=1, Docker and egret-plugin-host:dev"]
async fn plugin_tasks_run_end_to_end_and_no_secret_is_stored(pool: sqlx::PgPool) {
    gate();
    let pg = start_postgres().await;
    let fx = fx(&pool).await;
    fx.secret("pg", &[("username", "w5b"), ("password", PG_PASSWORD)])
        .await;

    // The comment makes the plugin's own DEBUG `Starting query: …` line echo
    // the secret; the result holds no secret (outputs are not masked, §12).
    let sql =
        "SELECT 41 + 1 AS answer, 'w5b-t6' AS tag /* {{ secret('pg', subkey='password') }} */";
    let exec = fx
        .run(
            &pg_flow("pg_e2e", pg.port, sql, "    fetchType: FETCH_ONE\n"),
            15,
            Duration::from_secs(120),
        )
        .await;
    let (state, outputs, message) = fx.task_run(exec, "q").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    assert_eq!(
        outputs,
        Some(serde_json::json!({"row": {"answer": 42, "tag": "w5b-t6"}, "size": 1})),
        "the rows are the outputs"
    );
    let lines = fx.api_lines(exec).await;
    println!("PG LINES: {lines:?}");
    let started = lines
        .iter()
        .find(|(_, m)| m.starts_with("Starting query:"))
        .unwrap_or_else(|| panic!("the plugin's own line came back through W11: {lines:?}"));
    assert_eq!(
        started.0, "DEBUG",
        "the host's level prefix is the stored level"
    );
    assert!(
        started.1.contains("/* ****** */"),
        "the echoed secret is masked: {started:?}"
    );
    assert!(lines.iter().all(|(_, m)| !m.contains(PG_PASSWORD)));
    let stored_raw: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow.task_logs WHERE strpos(message, $1) > 0")
            .bind(PG_PASSWORD)
            .fetch_one(&pool)
            .await
            .unwrap();
    let stored_masked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.task_logs WHERE execution_id = $1 AND strpos(message, '******') > 0",
    )
    .bind(exec)
    .fetch_one(&pool)
    .await
    .unwrap();
    println!("TASK_LOGS: {stored_raw} lines hold the value, {stored_masked} hold the mask");
    assert_eq!(stored_raw, 0);
    assert!(stored_masked >= 1);

    // ---- Trino (skippable) -------------------------------------------------
    let mut skipped = 0;
    match start_engine(TRINO_IMAGE, 8080, &["--memory", "3g"]).await {
        Err(why) => {
            println!("SKIPPED: Trino: {why}");
            skipped += 1;
        }
        Ok(trino) => {
            let deadline = Instant::now() + Duration::from_secs(180);
            let mut ready = false;
            while Instant::now() < deadline {
                let (ok, out) =
                    docker(&["exec", &trino.name, "trino", "--execute", "SELECT 1"]).await;
                if ok && out.contains('1') {
                    ready = true;
                    break;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            if !ready {
                println!("SKIPPED: Trino did not answer within 180 s");
                skipped += 1;
            } else {
                let src = format!(
                    "id: trino_e2e\nnamespace: w5b\ntasks:\n  - id: q\n    \
                     type: kestra.io.kestra.plugin.jdbc.trino.Query\n    \
                     url: jdbc:trino://host.docker.internal:{}/tpch/tiny\n    \
                     username: w5b\n    \
                     sql: \"SELECT count(*) AS n FROM nation\"\n    fetchType: FETCH_ONE\n",
                    trino.port
                );
                let exec = fx.run(&src, 15, Duration::from_secs(180)).await;
                let (state, outputs, message) = fx.task_run(exec, "q").await;
                println!("TRINO: {state} {outputs:?} {message:?}");
                assert_eq!(state, "SUCCESS", "{message:?}");
                assert_eq!(
                    outputs,
                    Some(serde_json::json!({"row": {"n": 25}, "size": 1}))
                );
            }
        }
    }

    // ---- StarRocks (skippable): the dev stack's FE on :9030 ------------------
    let sr_port = std::env::var("EGRET_TEST_STARROCKS_URL")
        .ok()
        .and_then(|u| {
            u.rsplit(':')
                .next()
                .and_then(|p| p.trim_end_matches('/').parse::<u16>().ok())
        });
    let sr_image_ok = docker(&["image", "inspect", STARROCKS_IMAGE]).await.0;
    match (sr_port, sr_image_ok) {
        (None, _) => {
            println!("SKIPPED: StarRocks: EGRET_TEST_STARROCKS_URL is not set");
            skipped += 1;
        }
        (_, false) => {
            println!("SKIPPED: StarRocks: the pinned image {STARROCKS_IMAGE} is not present");
            skipped += 1;
        }
        (Some(port), true) => {
            let src = format!(
                "id: sr_e2e\nnamespace: w5b\ntasks:\n  - id: q\n    \
                 type: kestra.io.kestra.plugin.jdbc.mysql.Query\n    \
                 url: jdbc:mysql://host.docker.internal:{port}/\n    \
                 username: root\n    \
                 sql: \"SELECT 6 * 7 AS answer\"\n    fetchType: FETCH_ONE\n"
            );
            let exec = fx.run(&src, 15, Duration::from_secs(120)).await;
            let (state, outputs, message) = fx.task_run(exec, "q").await;
            println!("STARROCKS: {state} {outputs:?} {message:?}");
            if state != "SUCCESS"
                && message
                    .as_deref()
                    .is_some_and(|m| m.contains("Communications link failure"))
            {
                println!("SKIPPED: StarRocks: not reachable on :{port}: {message:?}");
                skipped += 1;
            } else {
                assert_eq!(state, "SUCCESS", "{message:?}");
                assert_eq!(
                    outputs,
                    Some(serde_json::json!({"row": {"answer": 42}, "size": 1}))
                );
            }
        }
    }
    println!("ENGINES SKIPPED: {skipped} of 2 (Trino, StarRocks)");

    assert_no_secret_stored(&pool, PG_PASSWORD).await;
    drop(pg);
}

/// **A timed-out `pg_sleep(600)`**: FAILED with W11's `Timeout after …`, and
/// the query is gone from `pg_stat_activity` 20 s later -- the host's
/// shutdown hook called `kill()`, which Postgres honours (Task 1). The scan
/// closes it. (StarRocks's `kill()` blocks, spec §12: not asserted.)
#[sqlx::test(migrations = "../../migrations")]
#[ignore = "needs EGRET_PLUGIN_HOST_LIVE=1, Docker and egret-plugin-host:dev"]
async fn a_timed_out_query_is_cancelled_on_postgres(pool: sqlx::PgPool) {
    gate();
    let pg = start_postgres().await;
    let fx = fx(&pool).await;
    fx.secret("pg", &[("username", "w5b"), ("password", PG_PASSWORD)])
        .await;
    let started = Instant::now();
    let exec = fx
        .run(
            &pg_flow(
                "pg_sleep",
                pg.port,
                "SELECT pg_sleep(600)",
                "    timeout: PT5S\n",
            ),
            15,
            Duration::from_secs(120),
        )
        .await;
    println!("TIMEOUT: execution terminal after {:?}", started.elapsed());
    let (state, _, message) = fx.task_run(exec, "q").await;
    assert_eq!(state, "FAILED");
    assert_eq!(message.as_deref(), Some("Timeout after 00:00:05.000"));

    tokio::time::sleep(Duration::from_secs(20)).await;
    let sleeping = psql(
        &pg,
        "SELECT count(*) FROM pg_stat_activity WHERE query LIKE 'SELECT pg_sleep%' AND state = 'active'",
    )
    .await;
    let (_, log) = docker(&["logs", &pg.name]).await;
    let (_, err_log) = {
        let o = std::process::Command::new("docker")
            .args(["logs", &pg.name])
            .output()
            .expect("docker logs");
        (true, String::from_utf8_lossy(&o.stderr).to_string())
    };
    let cancelled = format!("{log}\n{err_log}").contains("canceling statement due to user request");
    println!("PG_STAT_ACTIVITY pg_sleep active 20 s later: {sleeping}; server log cancel line: {cancelled}");
    assert_eq!(
        sleeping.trim(),
        "0",
        "the query outlived its timed-out task"
    );
    assert!(
        cancelled,
        "the query was cancelled by the plugin's kill(), not by a dropped socket"
    );

    // Nothing of the attempt's container is left.
    let (_, left) = docker(&[
        "ps",
        "-a",
        "--filter",
        "name=egret-",
        "--format",
        "{{.Names}} {{.Image}}",
    ])
    .await;
    assert!(
        !left.lines().any(|l| l.contains(HOST_IMAGE)),
        "the plugin container was removed: {left}"
    );

    assert_no_secret_stored(&pool, PG_PASSWORD).await;
    drop(pg);
}
