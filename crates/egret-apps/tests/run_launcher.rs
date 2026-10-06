//! The subprocess launcher's run-pod support, with REAL processes (the same stand-in as
//! `subprocess_pod.rs`, `support/fake_marimo.py`) and the real database for pod tokens.

use std::path::PathBuf;
use std::time::Duration;

use egret_apps::launcher::{AppPodLauncher, RunPodKey};
use egret_apps::podtoken::PodTokenRepo;
use egret_apps::subprocess_pod::{SubprocessConfig, SubprocessPodLauncher};
use egret_core::UserId;
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
    app: Uuid,
}

impl Fixture {
    async fn new() -> Self {
        let url = egret_test_db::url().await.expect("test database");
        let db = Db::connect(&url, 5).await.expect("connect");
        let dir = std::env::temp_dir().join(format!("egret-runproc-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let tag = Uuid::new_v4().simple().to_string();
        let user: Uuid = sqlx::query_scalar(
            "INSERT INTO users (provider, email, name, status, password_hash) \
             VALUES ('local', $1, 'test', 'active', 'x') RETURNING id",
        )
        .bind(format!("runproc_{tag}@egret.local"))
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let ws: Uuid = sqlx::query_scalar(
            "INSERT INTO workspaces (slug, name, created_by, status) \
             VALUES ($1, 'Runproc', $2, 'active') RETURNING id",
        )
        .bind(format!("runproc-{tag}"))
        .bind(user)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let app: Uuid = sqlx::query_scalar(
            "INSERT INTO analytics.apps (workspace_id, name, owner_user_id) \
             VALUES ($1, 'app', $2) RETURNING id",
        )
        .bind(ws)
        .bind(user)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        Self {
            pods: PodTokenRepo::new(db.clone()),
            db,
            dir,
            app,
        }
    }

    /// A new published version of this fixture's one app.
    async fn version(&self) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO analytics.app_versions (app_id, version_no) \
             VALUES ($1, (SELECT coalesce(max(version_no),0)+1 FROM analytics.app_versions WHERE app_id = $1)) \
             RETURNING id",
        )
        .bind(self.app)
        .fetch_one(&self.db.pool)
        .await
        .unwrap()
    }

    async fn key(&self) -> RunPodKey {
        let tag = Uuid::new_v4().simple().to_string();
        let user: Uuid = sqlx::query_scalar(
            "INSERT INTO users (provider, email, name, status, password_hash) \
             VALUES ('local', $1, 'test', 'active', 'x') RETURNING id",
        )
        .bind(format!("runproc_viewer_{tag}@egret.local"))
        .fetch_one(&self.db.pool)
        .await
        .unwrap();
        RunPodKey {
            user_id: UserId(user),
            version_id: self.version().await,
        }
    }

    fn launcher(&self, command: Vec<String>) -> SubprocessPodLauncher {
        SubprocessPodLauncher::new(
            self.pods.clone(),
            SubprocessConfig {
                command,
                work_dir: self.dir.join("pods"),
                config_dir: self.dir.join("config"),
                venv_shim_dir: self.dir.join("venv-shim"),
                callback_url: "http://egret.test:8080".into(),
                run_kernel_mode: "thread".into(),
            },
        )
    }

    /// The stand-in marimo: `python3 fake_marimo.py` (the launcher appends `run`).
    fn marimo(&self) -> SubprocessPodLauncher {
        let script =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/fake_marimo.py");
        self.launcher(vec![python(), script.to_string_lossy().into_owned()])
    }

    fn env_of(&self, key: RunPodKey) -> serde_json::Value {
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
        .get(format!("{}r/{pod_ref}/health", ep.base))
        .bearer_auth(&ep.marimo_token)
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

#[tokio::test]
async fn a_run_pod_starts_with_its_version_and_app_id_and_the_r_prefixed_base_url() {
    let fx = Fixture::new().await;
    let l = fx.marimo();
    let key = fx.key().await;
    let ep = l.ensure_run(key, READY).await.unwrap();
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
    assert_eq!(get("EGRET_BASE_URL"), format!("/r/{}", key.pod_ref()));
    assert_eq!(get("EGRET_APP_ID"), fx.app.to_string());
    assert_eq!(get("EGRET_VERSION_ID"), key.version_id.to_string());
    assert_eq!(get("EGRET_RUN_KERNEL_MODE"), "thread");
    // the real launcher refuses to start a run pod without its venv shim, and outside the image
    // nothing but this variable can say where that is
    assert_eq!(
        get("EGRET_VENV_SHIM"),
        fx.dir.join("venv-shim").to_string_lossy()
    );
    // the pod token identifies the version, not the viewer (D24: not per-viewer)
    let principal = fx
        .pods
        .resolve(&get("EGRET_POD_TOKEN"))
        .await
        .unwrap()
        .expect("live pod token");
    assert_eq!(principal.user_id, None);
    assert_eq!(
        (principal.app_id, principal.version_id),
        (Some(fx.app), Some(key.version_id))
    );
    l.stop_run(key).await.unwrap();
}

#[tokio::test]
async fn two_viewers_of_the_same_version_get_two_pods_and_the_same_viewer_reuses_theirs() {
    let fx = Fixture::new().await;
    let l = std::sync::Arc::new(fx.marimo());
    let version = fx.version().await;
    let (user_a, user_b) = (UserId(Uuid::new_v4()), UserId(Uuid::new_v4()));
    // these two viewers' users were not inserted (not needed: RunPodKey carries no FK to users)
    let (ka, kb) = (
        RunPodKey {
            user_id: user_a,
            version_id: version,
        },
        RunPodKey {
            user_id: user_b,
            version_id: version,
        },
    );
    let ea = l.ensure_run(ka, READY).await.unwrap();
    let eb = l.ensure_run(kb, READY).await.unwrap();
    assert_ne!(
        ea.base, eb.base,
        "two different viewers, two different pods"
    );
    let ea2 = l.ensure_run(ka, READY).await.unwrap();
    assert_eq!(ea.base, ea2.base, "the same viewer reuses their own pod");
    l.stop_run(ka).await.unwrap();
    l.stop_run(kb).await.unwrap();
}

#[tokio::test]
async fn the_same_viewer_two_versions_gets_two_pods() {
    let fx = Fixture::new().await;
    let l = fx.marimo();
    let user = UserId(Uuid::new_v4());
    let (v1, v2) = (fx.version().await, fx.version().await);
    let e1 = l
        .ensure_run(
            RunPodKey {
                user_id: user,
                version_id: v1,
            },
            READY,
        )
        .await
        .unwrap();
    let e2 = l
        .ensure_run(
            RunPodKey {
                user_id: user,
                version_id: v2,
            },
            READY,
        )
        .await
        .unwrap();
    assert_ne!(e1.base, e2.base);
}

#[tokio::test]
async fn stopping_a_run_pod_kills_it_and_revokes_its_version_token_but_not_a_sibling_version() {
    let fx = Fixture::new().await;
    let l = fx.marimo();
    let key = fx.key().await;
    let other_key = RunPodKey {
        user_id: UserId(Uuid::new_v4()),
        version_id: fx.version().await,
    };
    let ep = l.ensure_run(key, READY).await.unwrap();
    l.ensure_run(other_key, READY).await.unwrap();
    let pod_token = fx.env_of(key)["EGRET_POD_TOKEN"]
        .as_str()
        .unwrap()
        .to_string();
    let other_token = fx.env_of(other_key)["EGRET_POD_TOKEN"]
        .as_str()
        .unwrap()
        .to_string();

    l.stop_run(key).await.unwrap();
    assert!(!healthy(&ep, &key.pod_ref()).await);
    assert!(
        fx.pods.resolve(&pod_token).await.unwrap().is_none(),
        "a copy of a dead run pod's token must be worth nothing"
    );
    assert!(
        fx.pods.resolve(&other_token).await.unwrap().is_some(),
        "a sibling version's token must survive"
    );
    l.stop_run(other_key).await.unwrap();
    l.stop_run(key)
        .await
        .expect("stopping a stopped pod is not an error");
}

#[tokio::test]
async fn ensure_run_fails_fast_and_cleanly_for_an_unknown_version() {
    let fx = Fixture::new().await;
    let l = fx.marimo();
    let key = RunPodKey {
        user_id: UserId(Uuid::new_v4()),
        version_id: Uuid::new_v4(), // never inserted
    };
    let err = l.ensure_run(key, READY).await.unwrap_err().to_string();
    assert!(err.contains("unknown version"), "{err}");
}

#[tokio::test]
async fn only_run_pods_unused_for_the_idle_time_are_reported_idle_and_separately_from_edit() {
    let fx = Fixture::new().await;
    let l = fx.marimo();
    let key = fx.key().await;
    l.ensure_run(key, READY).await.unwrap();
    assert!(l
        .idle_run_pods(Duration::from_secs(3600))
        .await
        .unwrap()
        .is_empty());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        l.idle_run_pods(Duration::from_millis(100)).await.unwrap(),
        [key]
    );
    assert!(
        l.idle_edit_pods(Duration::from_millis(100))
            .await
            .unwrap()
            .is_empty(),
        "a run pod must never show up as an idle edit pod"
    );
    l.ensure_run(key, READY).await.unwrap();
    assert!(l
        .idle_run_pods(Duration::from_millis(100))
        .await
        .unwrap()
        .is_empty());
    l.stop_run(key).await.unwrap();
}
