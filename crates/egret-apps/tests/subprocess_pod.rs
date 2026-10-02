//! The subprocess launcher with REAL processes (a stand-in for marimo, `support/fake_marimo.py`)
//! and the real database for pod tokens.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egret_apps::launcher::{AppPodLauncher, EditPodKey};
use egret_apps::podtoken::PodTokenRepo;
use egret_apps::subprocess_pod::{SubprocessConfig, SubprocessPodLauncher};
use egret_core::{UserId, WorkspaceId};
use egret_metastore::Db;
use uuid::Uuid;

const READY: Duration = Duration::from_secs(30);

fn python() -> String {
    std::env::var("PYTHON3").unwrap_or_else(|_| "python3".into())
}

struct Fixture {
    db: Db,
    pods: PodTokenRepo,
    dir: PathBuf,
}

impl Fixture {
    async fn new() -> Self {
        let url = egret_test_db::url().await.expect("test database");
        let db = Db::connect(&url, 5).await.expect("connect");
        let dir = std::env::temp_dir().join(format!("egret-subproc-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        Self {
            pods: PodTokenRepo::new(db.clone()),
            db,
            dir,
        }
    }

    async fn key(&self) -> EditPodKey {
        let tag = Uuid::new_v4().simple().to_string();
        let user: Uuid = sqlx::query_scalar(
            "INSERT INTO users (provider, email, name, status, password_hash) \
             VALUES ('local', $1, 'test', 'active', 'x') RETURNING id",
        )
        .bind(format!("subproc_{tag}@egret.local"))
        .fetch_one(&self.db.pool)
        .await
        .unwrap();
        let ws: Uuid = sqlx::query_scalar(
            "INSERT INTO workspaces (slug, name, created_by, status) \
             VALUES ($1, 'Subproc', $2, 'active') RETURNING id",
        )
        .bind(format!("subproc-{tag}"))
        .bind(user)
        .fetch_one(&self.db.pool)
        .await
        .unwrap();
        EditPodKey {
            user_id: UserId(user),
            workspace_id: WorkspaceId(ws),
        }
    }

    fn launcher(&self, command: Vec<String>) -> SubprocessPodLauncher {
        SubprocessPodLauncher::new(
            self.pods.clone(),
            SubprocessConfig {
                command,
                work_dir: self.dir.join("pods"),
                config_dir: self.dir.join("config"),
                callback_url: "http://egret.test:8080".into(),
            },
        )
    }

    /// The stand-in marimo: `python3 fake_marimo.py` (the launcher appends `edit`).
    fn marimo(&self) -> SubprocessPodLauncher {
        let script =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/fake_marimo.py");
        self.launcher(vec![python(), script.to_string_lossy().into_owned()])
    }

    fn env_of(&self, key: EditPodKey) -> serde_json::Value {
        let p = self
            .dir
            .join("pods")
            .join(key.pod_ref())
            .join("work/env.json");
        serde_json::from_str(&std::fs::read_to_string(p).expect("env.json")).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn healthy(ep: &egret_apps::PodEndpoint, pod_ref: &str) -> bool {
    reqwest::Client::new()
        .get(format!("{}e/{pod_ref}/health", ep.base))
        .bearer_auth(&ep.marimo_token)
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

#[tokio::test]
async fn a_pod_starts_with_exactly_the_environment_the_contract_names() {
    let fx = Fixture::new().await;
    let l = fx.marimo();
    let key = fx.key().await;
    let ep = l.ensure_edit(key, READY).await.unwrap();
    assert!(
        healthy(&ep, &key.pod_ref()).await,
        "answers its health check behind its token"
    );

    let env = fx.env_of(key);
    let get = |k: &str| {
        env[k]
            .as_str()
            .unwrap_or_else(|| panic!("{k} missing in {env}"))
            .to_string()
    };
    assert_eq!(get("EGRET_URL"), "http://egret.test:8080");
    assert_eq!(get("EGRET_BASE_URL"), format!("/e/{}", key.pod_ref()));
    assert_eq!(get("EGRET_MARIMO_TOKEN"), ep.marimo_token);
    // the pod token is real, and is the author's edit token
    let principal = fx
        .pods
        .resolve(&get("EGRET_POD_TOKEN"))
        .await
        .unwrap()
        .expect("live pod token");
    assert_eq!(principal.user_id, Some(key.user_id));
    assert_eq!(principal.workspace_id, key.workspace_id);
    // the parent's environment (the test process holds DATABASE_URL) is NOT inherited
    for leaked in ["DATABASE_URL", "EGRET_SECRETS__MASTER_KEY", "HOME_PARENT"] {
        assert!(env.get(leaked).is_none(), "{leaked} leaked into the pod");
    }
    l.stop_edit(key).await.unwrap();
}

#[tokio::test]
async fn a_second_request_reuses_the_pod_and_two_at_once_start_one() {
    let fx = Fixture::new().await;
    let l = std::sync::Arc::new(fx.marimo());
    let key = fx.key().await;
    let (a, b) = tokio::join!(
        {
            let l = l.clone();
            async move { l.ensure_edit(key, READY).await.unwrap() }
        },
        {
            let l = l.clone();
            async move { l.ensure_edit(key, READY).await.unwrap() }
        },
    );
    assert_eq!(
        a.base, b.base,
        "one process for two simultaneous cold requests"
    );
    let c = l.ensure_edit(key, READY).await.unwrap();
    assert_eq!(a.base, c.base);
    assert_eq!(a.marimo_token, c.marimo_token);
    l.stop_edit(key).await.unwrap();
}

#[tokio::test]
async fn two_authors_get_two_pods_and_stopping_one_leaves_the_other() {
    let fx = Fixture::new().await;
    let l = fx.marimo();
    let (k1, k2) = (fx.key().await, fx.key().await);
    let e1 = l.ensure_edit(k1, READY).await.unwrap();
    let e2 = l.ensure_edit(k2, READY).await.unwrap();
    assert_ne!(e1.base, e2.base);
    assert_ne!(e1.marimo_token, e2.marimo_token);
    l.stop_edit(k1).await.unwrap();
    assert!(!healthy(&e1, &k1.pod_ref()).await, "k1's pod is gone");
    assert!(healthy(&e2, &k2.pod_ref()).await, "k2's pod is untouched");
    l.stop_edit(k2).await.unwrap();
}

#[tokio::test]
async fn stopping_a_pod_kills_it_and_revokes_its_token() {
    let fx = Fixture::new().await;
    let l = fx.marimo();
    let key = fx.key().await;
    let ep = l.ensure_edit(key, READY).await.unwrap();
    let pod_token = fx.env_of(key)["EGRET_POD_TOKEN"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(fx.pods.resolve(&pod_token).await.unwrap().is_some());

    l.stop_edit(key).await.unwrap();
    assert!(!healthy(&ep, &key.pod_ref()).await);
    assert!(
        fx.pods.resolve(&pod_token).await.unwrap().is_none(),
        "a copy of a dead pod's token must be worth nothing"
    );
    l.stop_edit(key)
        .await
        .expect("stopping a stopped pod is not an error");
}

#[tokio::test]
async fn a_pod_that_died_by_itself_is_replaced_and_its_token_revoked() {
    let fx = Fixture::new().await;
    let l = fx.marimo();
    let key = fx.key().await;
    let first = l.ensure_edit(key, READY).await.unwrap();
    let old_token = fx.env_of(key)["EGRET_POD_TOKEN"]
        .as_str()
        .unwrap()
        .to_string();
    // kill it behind the launcher's back
    let port = first.base.port().unwrap();
    let out = std::process::Command::new("lsof")
        .args(["-t", "-i", &format!("tcp:{port}"), "-sTCP:LISTEN"])
        .output()
        .unwrap();
    let pid = String::from_utf8_lossy(&out.stdout)
        .trim()
        .lines()
        .next()
        .unwrap()
        .to_string();
    std::process::Command::new("kill")
        .args(["-KILL", &pid])
        .status()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let second = l.ensure_edit(key, READY).await.unwrap();
    assert_ne!(first.base, second.base, "a new process");
    assert!(healthy(&second, &key.pod_ref()).await);
    assert!(
        fx.pods.resolve(&old_token).await.unwrap().is_none(),
        "the dead pod's token is revoked"
    );
    let new_token = fx.env_of(key)["EGRET_POD_TOKEN"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(fx.pods.resolve(&new_token).await.unwrap().is_some());
    l.stop_edit(key).await.unwrap();
}

#[tokio::test]
async fn a_pod_that_exits_at_start_fails_fast_with_its_own_words_and_leaves_no_token() {
    let fx = Fixture::new().await;
    let l = fx.launcher(vec![
        python(),
        "-c".into(),
        "import sys; sys.stderr.write('boom: bad environment\\n'); sys.exit(3)".into(),
    ]);
    let key = fx.key().await;
    let t0 = Instant::now();
    let err = l
        .ensure_edit(key, Duration::from_secs(30))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("exited") && err.contains("boom: bad environment"),
        "{err}"
    );
    assert!(
        t0.elapsed() < Duration::from_secs(10),
        "did not wait out the timeout"
    );
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM analytics.app_pod_tokens WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(key.user_id.0)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(live, 0);
}

#[tokio::test]
async fn a_pod_that_never_answers_is_killed_at_the_timeout() {
    let fx = Fixture::new().await;
    let l = fx.launcher(vec![
        python(),
        "-c".into(),
        "import time; time.sleep(3600)".into(),
    ]);
    let key = fx.key().await;
    let t0 = Instant::now();
    let err = l
        .ensure_edit(key, Duration::from_secs(2))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("not ready within"), "{err}");
    assert!(t0.elapsed() < Duration::from_secs(10));
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM analytics.app_pod_tokens WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(key.user_id.0)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(live, 0);
}

#[tokio::test]
async fn only_pods_unused_for_the_idle_time_are_reported_idle() {
    let fx = Fixture::new().await;
    let l = fx.marimo();
    let key = fx.key().await;
    l.ensure_edit(key, READY).await.unwrap();
    assert!(l
        .idle_edit_pods(Duration::from_secs(3600))
        .await
        .unwrap()
        .is_empty());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        l.idle_edit_pods(Duration::from_millis(100)).await.unwrap(),
        [key]
    );
    // asking for the pod is what makes it "in use" again
    l.ensure_edit(key, READY).await.unwrap();
    assert!(l
        .idle_edit_pods(Duration::from_millis(100))
        .await
        .unwrap()
        .is_empty());
    l.stop_edit(key).await.unwrap();
}
