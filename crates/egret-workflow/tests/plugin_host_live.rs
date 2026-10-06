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
use egret_workflow::storage::FileStore;
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

/// The raw output of the first container labelled with `exec`, followed from
/// the moment it appears until it stops (`docker logs -f`). Empty if none
/// appeared within two minutes -- the caller asserts on content, so an empty
/// capture fails it rather than passing vacuously.
async fn container_output(exec: Uuid) -> String {
    let label = format!("label=egret.io/execution-id={exec}");
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let (_, ids) = docker(&["ps", "-aq", "--filter", &label]).await;
        let id = ids
            .lines()
            .next()
            .filter(|l| !l.is_empty())
            .map(str::to_string);
        // Not while it is only `created`: `docker logs -f` on a container that
        // has not started returns at once, empty.
        let started = match &id {
            Some(id) => {
                let (_, status) = docker(&["inspect", "-f", "{{.State.Status}}", id]).await;
                status != "created"
            }
            None => false,
        };
        if let (Some(id), true) = (id.as_deref(), started) {
            let out = Command::new("docker")
                .args(["logs", "-f", id])
                .output()
                .await
                .expect("docker logs");
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&out.stderr));
            return text;
        }
        if Instant::now() > deadline {
            return String::new();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
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
    start_engine_with(image, inner, extra, &[]).await
}

/// [`start_engine`] with `cmd` after the image: the server's own arguments.
async fn start_engine_with(
    image: &str,
    inner: u16,
    extra: &[&str],
    cmd: &[&str],
) -> Result<Engine, String> {
    let name = format!("egret-w5b-t6-{}", Uuid::new_v4().simple());
    let port_spec = format!("127.0.0.1::{inner}");
    let mut args = vec!["run", "-d", "--name", &name, "-p", &port_spec];
    args.extend_from_slice(extra);
    args.push(image);
    args.extend_from_slice(cmd);
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
    start_postgres_with(&[]).await
}

/// [`start_postgres`] with the server's `-c name=value` arguments.
async fn start_postgres_with(server_args: &[&str]) -> Engine {
    gate();
    let pw = format!("POSTGRES_PASSWORD={PG_PASSWORD}");
    let pg = start_engine_with(
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
        server_args,
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
    /// Internal storage for the runner and the plugin bridge (W9a Task 9);
    /// `None` is W5b's shape.
    files: Option<Arc<FileStore>>,
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
        files: None,
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
        let mut h = Handlers::with_core_tasks(egret_workflow::storage::current());
        register(
            &mut h,
            Arc::new(KestraRuntime {
                runner: Arc::new({
                    let r = DockerRunner::new()
                        .with_logs(egret_workflow::logs::LogRepo::new(self.db.pool.clone()));
                    match &self.files {
                        Some(files) => r.with_files(files.clone()),
                        None => r,
                    }
                }),
                runner_kind: RunnerKind::Docker,
                features: Arc::new(PgFeatureState::new(self.db.pool.clone())),
                plugin_configurations: Default::default(),
                host: egret_config::PluginHostConfig {
                    image: Some(HOST_IMAGE.into()),
                    kill_grace_s,
                    ..Default::default()
                },
                secrets: Arc::new(StoreSource(self.store.clone())),
                files: self.files.clone(),
            }),
        );
        h
    }

    /// Run `source` to a terminal state through `spawn_with_logs`, then stop
    /// the runtime (its join drains the log writer).
    async fn run(&self, source: &str, kill_grace_s: u64, within: Duration) -> Uuid {
        self.run_capturing(source, kill_grace_s, within, false)
            .await
            .0
    }

    /// [`Fx::run`], and with `capture`, what the plugin container printed as
    /// Docker keeps it (`docker logs -f`, stdout and stderr, read while the
    /// container exists): the text `kubectl logs` or a log shipper would see
    /// (fix round 1, I1).
    async fn run_capturing(
        &self,
        source: &str,
        kill_grace_s: u64,
        within: Duration,
        capture: bool,
    ) -> (Uuid, Option<String>) {
        let exec = start_execution(&self.db.pool, self.ws, None, source, serde_json::json!({}))
            .await
            .expect("start execution");
        let captured = capture.then(|| tokio::spawn(container_output(exec)));
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
        let output = match captured {
            Some(h) => Some(
                tokio::time::timeout(Duration::from_secs(60), h)
                    .await
                    .expect("docker logs ends")
                    .expect("the capture task"),
            ),
            None => None,
        };
        (exec, output)
    }

    async fn task_run(
        &self,
        exec: Uuid,
        task: &str,
    ) -> (String, Option<serde_json::Value>, Option<String>) {
        sqlx::query_as(
            "SELECT state, outputs, message FROM workflow.task_runs \
              WHERE execution_id = $1 AND task_id = $2",
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

/// An unexported `DATABASE_URL` does not skip the `#[sqlx::test]`s below — it
/// silently points them at the database `.env` names, which is the DEV one.
/// Not `#[ignore]`d, so the plain `cargo test` run checks it. See
/// `egret_test_db::assert_database_url_is_exported`.
#[test]
fn database_url_is_exported_rather_than_read_from_dotenv() {
    egret_test_db::assert_database_url_is_exported();
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
    // the secret; the result holds no secret (outputs are not masked, §12 --
    // and the username secret `w5b` is inside the returned tag, which the
    // outputs must keep).
    let sql =
        "SELECT 41 + 1 AS answer, 'w5b-t6' AS tag /* {{ secret('pg', subkey='password') }} */";
    let (exec, raw) = fx
        .run_capturing(
            &pg_flow("pg_e2e", pg.port, sql, "    fetchType: FETCH_ONE\n"),
            15,
            Duration::from_secs(120),
            true,
        )
        .await;
    // Fix round 1, I1: the container's own stdout -- what `kubectl logs`,
    // a node's log shipper and Docker's json-file driver keep -- carries the
    // masked line and never the value.
    let raw = raw.expect("captured");
    println!("CONTAINER STDOUT: {raw}");
    assert!(
        raw.contains("/* ****** */"),
        "the capture saw the host's masked line: {raw}"
    );
    assert!(
        raw.contains(r#""tag":"w5b-t6""#),
        "the outputs marker is the one line the host leaves as it is: {raw}"
    );
    assert_eq!(
        raw.matches(PG_PASSWORD).count(),
        0,
        "the password reached the container's stdout: {raw}"
    );
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
///
/// **The budget is 30 s, not 5 s, and the server log must show the query
/// was sent.** The budget covers the JVM's boot: ~3.6 s to the plugin's
/// connect on an idle machine, over 5 s at load average 35-49 (W5b Task 6
/// fix 1, measured). A SIGTERM that lands before the host registered its
/// hook ends the JVM (exit 143) with nothing sent -- nothing to cancel,
/// correctly -- and a 5 s budget then failed this test on "no cancel line"
/// 10 runs in 16 while measuring nothing. Hence the `sent` assertion first:
/// a boot slower than the budget fails as that, never as a missing cancel.
#[sqlx::test(migrations = "../../migrations")]
#[ignore = "needs EGRET_PLUGIN_HOST_LIVE=1, Docker and egret-plugin-host:dev"]
async fn a_timed_out_query_is_cancelled_on_postgres(pool: sqlx::PgPool) {
    gate();
    // Every statement and connection in the server's log, with times: whether
    // the query was ever SENT is read from it, not inferred.
    let pg = start_postgres_with(&[
        "-c",
        "log_statement=all",
        "-c",
        "log_connections=on",
        "-c",
        "log_disconnections=on",
        "-c",
        "log_line_prefix=%m [%p] ",
    ])
    .await;
    let fx = fx(&pool).await;
    fx.secret("pg", &[("username", "w5b"), ("password", PG_PASSWORD)])
        .await;
    let started = Instant::now();
    println!("EXEC START (UTC): {}", chrono::Utc::now());
    let exec = fx
        .run(
            &pg_flow(
                "pg_sleep",
                pg.port,
                "SELECT pg_sleep(600)",
                "    timeout: PT30S\n",
            ),
            15,
            Duration::from_secs(120),
        )
        .await;
    println!("TIMEOUT: execution terminal after {:?}", started.elapsed());
    let (state, _, message) = fx.task_run(exec, "q").await;
    assert_eq!(state, "FAILED");
    assert_eq!(message.as_deref(), Some("Timeout after 00:00:30.000"));

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
    let server_log = format!("{log}\n{err_log}");
    let cancelled = server_log.contains("canceling statement due to user request");
    let sent = server_log
        .lines()
        .filter(|l| l.contains("execute <unnamed>: SELECT pg_sleep(600)"))
        .count();
    println!("PG_STAT_ACTIVITY pg_sleep active 20 s later: {sleeping}; server log cancel line: {cancelled}; query sent: {sent}");
    for l in server_log.lines().filter(|l| {
        l.contains("host=172.") || l.contains("execute <unnamed>") || l.contains("canceling")
    }) {
        println!("PG LOG: {l}");
    }
    assert_eq!(
        sleeping.trim(),
        "0",
        "the query outlived its timed-out task"
    );
    assert_eq!(
        sent, 1,
        "the query was never sent: the budget ran out before the JVM reached it, so there \
         was nothing to cancel and this run measured nothing"
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

// ---------------------------------------------------------------------------
// W9a Task 9: plugins read and write internal storage through egret
// ---------------------------------------------------------------------------

/// RustFS under a throwaway prefix `w9a-t9-test/<uuid>/`, from the four
/// `EGRET_TEST_S3_*` (the gate: this test is RustFS-gated and FAILS without
/// them, as it does without [`GATE`]).
async fn rustfs_files() -> Arc<FileStore> {
    let need = |k: &str| {
        std::env::var(k).unwrap_or_else(|_| {
            panic!("set {k} (and the other EGRET_TEST_S3_*) to run the plugin storage live test")
        })
    };
    let cfg = egret_storage::StorageConfig {
        endpoint: Some(need("EGRET_TEST_S3_ENDPOINT")),
        region: "us-east-1".into(),
        bucket: need("EGRET_TEST_S3_BUCKET"),
        access_key: need("EGRET_TEST_S3_KEY"),
        secret_key: need("EGRET_TEST_S3_SECRET"),
        path_style: true,
    };
    let store = egret_storage::S3Store::new(&cfg)
        .await
        .expect("an S3 store");
    Arc::new(FileStore {
        store: Arc::new(store),
        bucket: cfg.bucket,
        prefix: format!("w9a-t9-test/{}/", Uuid::new_v4()),
        caps: egret_workflow::storage::Caps::default(),
        retention_days: 30,
    })
}

/// Delete, by exact key, every object under `files`' throwaway prefix --
/// never a prefix delete, never outside `w9a-t9-test/`. Returns the count.
async fn forget(files: &FileStore) -> usize {
    assert!(files.prefix.starts_with("w9a-t9-test/"), "{}", files.prefix);
    let listed = files.store.list(&files.prefix, None).await.expect("list");
    for o in &listed.objects {
        assert!(o.key.starts_with(&files.prefix), "{}", o.key);
        files
            .store
            .delete(&format!("s3://{}/{}", files.bucket, o.key))
            .await
            .expect("delete by exact key");
    }
    listed.objects.len()
}

/// The three connection lines of a Postgres plugin task at `port`.
fn pg_conn(port: u16) -> String {
    format!(
        "    url: jdbc:postgresql://host.docker.internal:{port}/w5b\n    \
         username: \"{{{{ secret('pg', subkey='username') }}}}\"\n    \
         password: \"{{{{ secret('pg', subkey='password') }}}}\"\n"
    )
}

/// `docker exec … mysql -e` against a throwaway StarRocks.
async fn sr_sql(sr: &Engine, sql: &str) -> (bool, String) {
    docker(&[
        "exec",
        &sr.name,
        "mysql",
        "-h127.0.0.1",
        "-P9030",
        "-uroot",
        "-N",
        "-e",
        sql,
    ])
    .await
}

/// **Kestra plugins read and write internal storage through egret** (W9a
/// spec §9.1), end to end through the real worker, `DockerRunner`, the real
/// host image, a throwaway Postgres and RustFS:
///
/// `postgresql.Query` `fetchType: STORE` -> `postgresql.Batch` from that
/// (binary Ion) URI -> `postgresql.CopyOut` (CSV) -> `postgresql.CopyIn` from
/// the CopyOut URI -> a `Log` with `read()` of the CopyOut URI. Then
/// `mysql.Query` STORE -> `mysql.Batch` on a THROWAWAY StarRocks (skipped,
/// printed, without `EGRET_TEST_STARROCKS_URL` or the pinned image). A file
/// written by a STORE that selects the secret holds its value: files a plugin
/// writes are NOT masked (W5b §12, W9a §9.1) -- recorded and printed here,
/// not asserted away.
///
/// `multi_thread` with `common::db()`: `read()` blocks the render thread on
/// the runtime handle (global constraints); `#[sqlx::test]` is
/// current-thread.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs EGRET_PLUGIN_HOST_LIVE=1, Docker, egret-plugin-host:dev and RustFS (EGRET_TEST_S3_*)"]
async fn plugins_read_and_write_storage_through_egret() {
    gate();
    let files = rustfs_files().await;
    if let Err(other) = egret_workflow::storage::install(files.clone()) {
        panic!("another store is installed in this process: {other:?}");
    }
    let db = common::db().await.expect("DATABASE_URL");
    let pool = db.pool.clone();
    let pg = start_postgres().await;
    psql(
        &pg,
        "CREATE TABLE src (i int, t text); INSERT INTO src VALUES (20, 'p'), (21, 'q'), (22, 'r'); \
         CREATE TABLE dst (i int, t text); CREATE TABLE dst2 (i int, t text)",
    )
    .await;
    let mut fx = fx(&pool).await;
    fx.files = Some(files.clone());
    fx.secret("pg", &[("username", "w5b"), ("password", PG_PASSWORD)])
        .await;
    let conn = pg_conn(pg.port);
    let src = format!(
        "id: pg_storage\nnamespace: w9a.plugins\ntasks:\n\
         \x20 - id: store\n    type: kestra.io.kestra.plugin.jdbc.postgresql.Query\n{conn}    \
         sql: \"SELECT i, t FROM src ORDER BY i\"\n    fetchType: STORE\n\
         \x20 - id: batch\n    type: kestra.io.kestra.plugin.jdbc.postgresql.Batch\n{conn}    \
         from: \"{{{{ outputs.store.uri }}}}\"\n    sql: \"INSERT INTO dst (i, t) VALUES (?, ?)\"\n\
         \x20 - id: copyout\n    type: kestra.io.kestra.plugin.jdbc.postgresql.CopyOut\n{conn}    \
         sql: \"SELECT i, t FROM dst ORDER BY i\"\n    format: CSV\n    header: true\n\
         \x20 - id: copyin\n    type: kestra.io.kestra.plugin.jdbc.postgresql.CopyIn\n{conn}    \
         table: dst2\n    from: \"{{{{ outputs.copyout.uri }}}}\"\n    format: CSV\n    header: true\n\
         \x20 - id: show\n    type: io.kestra.plugin.core.log.Log\n    \
         message: \"{{{{ read(outputs.copyout.uri) }}}}\"\n\
         \x20 - id: leak\n    type: kestra.io.kestra.plugin.jdbc.postgresql.Query\n{conn}    \
         sql: \"SELECT '{{{{ secret('pg', subkey='password') }}}}' AS pw\"\n    fetchType: STORE\n"
    );
    let exec = fx.run(&src, 15, Duration::from_secs(300)).await;
    let mut uris = BTreeMap::new();
    for task in ["store", "batch", "copyout", "copyin", "show", "leak"] {
        let (state, outputs, message) = fx.task_run(exec, task).await;
        println!("PG {task}: {state} {outputs:?} {message:?}");
        assert_eq!(state, "SUCCESS", "{task}: {message:?}");
        if let Some(uri) = outputs.as_ref().and_then(|o| o["uri"].as_str()) {
            uris.insert(task, uri.to_string());
        }
    }
    // The STORE file and the CopyOut file are this run's, under each task
    // run's own prefix, and indexed.
    let (_, store_out, _) = fx.task_run(exec, "store").await;
    assert_eq!(store_out.as_ref().unwrap()["size"], 3, "three rows stored");
    let (_, batch_out, _) = fx.task_run(exec, "batch").await;
    assert_eq!(batch_out.as_ref().unwrap()["rowCount"], 3);
    let (_, copyin_out, _) = fx.task_run(exec, "copyin").await;
    assert_eq!(copyin_out.as_ref().unwrap()["rowCount"], 3);
    for (task, uri) in &uris {
        assert!(
            uri.starts_with(&format!(
                "kestra:///w9a/plugins/pg-storage/executions/{exec}/tasks/{task}/"
            )),
            "{task}: {uri}"
        );
        let indexed: Option<i64> = sqlx::query_scalar(
            "SELECT size FROM workflow.execution_files WHERE workspace_id = $1 AND uri = $2",
        )
        .bind(fx.ws)
        .bind(uri)
        .fetch_optional(&pool)
        .await
        .unwrap();
        println!("INDEXED {task}: {uri} ({indexed:?} bytes)");
        assert!(indexed.is_some_and(|n| n > 0), "{uri} is indexed");
    }
    assert!(uris["copyout"].ends_with("/copy-out"), "Kestra's own name");
    assert_eq!(psql(&pg, "SELECT count(*) FROM dst").await.trim(), "3");
    assert_eq!(
        psql(
            &pg,
            "SELECT string_agg(i || ':' || t, ',' ORDER BY i) FROM dst2"
        )
        .await
        .trim(),
        "20:p,21:q,22:r",
        "Query STORE -> Batch -> CopyOut -> CopyIn carried every row"
    );
    let lines = fx.api_lines(exec).await;
    assert!(
        lines
            .iter()
            .any(|(_, m)| m.contains("i,t\n20,p\n21,q\n22,r")),
        "the Log read the CopyOut file: {lines:?}"
    );

    // Files a plugin writes are NOT masked (W5b §12, W9a §9.1): recorded.
    let leak = &uris["leak"];
    let bytes = files
        .store
        .get(&files.object_uri(fx.ws, leak).unwrap())
        .await
        .expect("the leak file is in the store");
    let held = bytes
        .windows(PG_PASSWORD.len())
        .filter(|w| *w == PG_PASSWORD.as_bytes())
        .count();
    println!(
        "UNMASKED FILE (by design, W5b §12 / W9a §9.1): {leak} ({} bytes) holds the secret's \
         value {held} time(s)",
        bytes.len()
    );
    assert_eq!(
        held, 1,
        "recorded: the STORE file holds the selected secret"
    );
    assert_no_secret_stored(&pool, PG_PASSWORD).await;
    drop(pg);

    let mut skipped = 0;
    // ---- Trino (skippable): `trino.Query fetchType: STORE` ---------------------
    match start_engine(TRINO_IMAGE, 8080, &["--memory", "3g"]).await {
        Err(why) => {
            println!("SKIPPED: Trino STORE: {why}");
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
                println!("SKIPPED: Trino STORE: Trino did not answer within 180 s");
                skipped += 1;
            } else {
                let src = format!(
                    "id: trino_storage\nnamespace: w9a.plugins\ntasks:\n  - id: q\n    \
                     type: kestra.io.kestra.plugin.jdbc.trino.Query\n    \
                     url: jdbc:trino://host.docker.internal:{}/tpch/tiny\n    \
                     username: w9a\n    \
                     sql: \"SELECT nationkey, name FROM nation ORDER BY nationkey\"\n    \
                     fetchType: STORE\n",
                    trino.port
                );
                let exec = fx.run(&src, 15, Duration::from_secs(180)).await;
                let (state, outputs, message) = fx.task_run(exec, "q").await;
                println!("TRINO STORE: {state} {outputs:?} {message:?}");
                assert_eq!(state, "SUCCESS", "{message:?}");
                let outputs = outputs.expect("outputs");
                assert_eq!(outputs["size"], 25, "tpch tiny has 25 nations");
                let uri = outputs["uri"].as_str().expect("a uri");
                let indexed: Option<i64> = sqlx::query_scalar(
                    "SELECT size FROM workflow.execution_files WHERE workspace_id = $1 AND uri = $2",
                )
                .bind(fx.ws)
                .bind(uri)
                .fetch_optional(&pool)
                .await
                .unwrap();
                println!("INDEXED trino: {uri} ({indexed:?} bytes)");
                assert!(indexed.is_some_and(|n| n > 0), "{uri} is indexed");
            }
        }
    }

    // ---- StarRocks (skippable), a THROWAWAY container ------------------------
    let sr_image_ok = docker(&["image", "inspect", STARROCKS_IMAGE]).await.0;
    if std::env::var("EGRET_TEST_STARROCKS_URL").is_err() {
        println!("SKIPPED: StarRocks mysql.Batch: EGRET_TEST_STARROCKS_URL is not set");
        skipped += 1;
    } else if !sr_image_ok {
        println!(
            "SKIPPED: StarRocks mysql.Batch: the pinned image {STARROCKS_IMAGE} is not present"
        );
        skipped += 1;
    } else {
        let sr = start_engine(STARROCKS_IMAGE, 9030, &[])
            .await
            .expect("a throwaway StarRocks starts");
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            let (ok, out) = sr_sql(&sr, "SHOW BACKENDS").await;
            if ok && out.contains("true") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "StarRocks did not come up: {out}"
            );
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
        let (ok, out) = sr_sql(
            &sr,
            "CREATE DATABASE w9a; CREATE TABLE w9a.t (i INT, t VARCHAR(8)) \
             DISTRIBUTED BY HASH(i) BUCKETS 1 PROPERTIES (\"replication_num\" = \"1\")",
        )
        .await;
        assert!(ok, "{out}");
        let url = format!("jdbc:mysql://host.docker.internal:{}/", sr.port);
        let src = format!(
            "id: sr_storage\nnamespace: w9a.plugins\ntasks:\n\
             \x20 - id: q\n    type: kestra.io.kestra.plugin.jdbc.mysql.Query\n    url: {url}\n    \
             username: root\n    \
             sql: \"SELECT 20 AS i, 'p' AS t UNION ALL SELECT 21, 'q' UNION ALL SELECT 22, 'r'\"\n    \
             fetchType: STORE\n\
             \x20 - id: b\n    type: kestra.io.kestra.plugin.jdbc.mysql.Batch\n    url: {url}\n    \
             username: root\n    from: \"{{{{ outputs.q.uri }}}}\"\n    \
             sql: \"INSERT INTO w9a.t (i, t) VALUES (?, ?)\"\n"
        );
        let exec = fx.run(&src, 15, Duration::from_secs(300)).await;
        for task in ["q", "b"] {
            let (state, outputs, message) = fx.task_run(exec, task).await;
            println!("STARROCKS {task}: {state} {outputs:?} {message:?}");
            assert_eq!(state, "SUCCESS", "{task}: {message:?}");
        }
        let (ok, rows) = sr_sql(&sr, "SELECT i, t FROM w9a.t ORDER BY i").await;
        println!("STARROCKS rows: {rows:?}");
        assert!(ok, "{rows}");
        assert_eq!(
            rows.lines().map(str::trim).collect::<Vec<_>>(),
            vec!["20\tp", "21\tq", "22\tr"]
        );
        drop(sr);
    }
    println!("ENGINES SKIPPED: {skipped} of 2 (Trino STORE, StarRocks mysql.Batch)");

    let removed = forget(&files).await;
    let left = files
        .store
        .list(&files.prefix, None)
        .await
        .unwrap()
        .objects
        .len();
    println!(
        "RUSTFS: {removed} objects deleted by exact key under {}; {left} left",
        files.prefix
    );
    assert!(removed >= 4, "the run wrote files to RustFS: {removed}");
    assert_eq!(left, 0);
}
