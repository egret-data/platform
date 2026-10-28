//! Fake Ranger plugins for the hold tests: a task that polls egret's real
//! download route the way a plugin does and records each poll. Shared by
//! `view_hold_db.rs` and `table_hook_db.rs` through `#[path]`.

// Each test binary that includes this uses a different subset of it.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::StatusCode;
use egret_policy::{ranger_router, PolicyStore, RangerState};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;

/// The operator token the fake plugins poll with.
pub const RANGER_TOKEN: &str = "devtoken";

/// The fake Trino plugin of `service`: polls `download_tags` every `poll`,
/// reporting the `tagVersion` its last 200 carried, and, under the same plugin
/// id, `/plugins/policies/download/{service}`, reporting the `policyVersion`
/// its last 200 carried (Trino 483 sends `lastKnownVersion` on its policy
/// polls too: grants-follow spec E26). `polls` records the tag polls,
/// `policy_polls` the policy polls.
pub fn start_trino(policy: PolicyStore, service: String, poll: Duration) -> Plugin {
    let plugin_id = format!("trino@vh-coordinator-abc12-x9z8q-{service}");
    let mut tags = start(
        policy.clone(),
        format!("/tags/download/{service}"),
        plugin_id.clone(),
        "tagVersion",
        poll,
    );
    let policies = start(
        policy,
        format!("/plugins/policies/download/{service}"),
        plugin_id,
        "policyVersion",
        poll,
    );
    tags.policy_polls = policies.polls.clone();
    tags.others.push(policies);
    tags
}

/// The fake StarRocks plugin of `service`: polls
/// `/plugins/policies/download/{service}` every `poll`, reporting the
/// `policyVersion` its last 200 carried (StarRocks sends `pluginId` and
/// `lastKnownVersion` on its policy polls, spec E24).
pub fn start_starrocks(policy: PolicyStore, service: String, poll: Duration) -> Plugin {
    let plugin_id = format!("starrocks@sr-fe-0-{service}");
    start(
        policy,
        format!("/plugins/policies/download/{service}"),
        plugin_id,
        "policyVersion",
        poll,
    )
}

fn start(
    policy: PolicyStore,
    path: String,
    plugin_id: String,
    version_field: &'static str,
    poll: Duration,
) -> Plugin {
    let polls: Arc<Mutex<Vec<(Instant, i64)>>> = Arc::default();
    let router = ranger_router(RangerState {
        store: policy,
        auth_token: Arc::from(RANGER_TOKEN),
        mode: egret_policy::ranger::AuthMode::Required,
        verifier: None,
    });
    let log = polls.clone();
    let id = plugin_id.clone();
    let task = tokio::spawn(async move {
        let mut held: i64 = -1;
        loop {
            let uri = format!("{path}?pluginId={id}&lastKnownVersion={held}");
            let req = axum::http::Request::builder()
                .uri(uri)
                .header("authorization", format!("Bearer {RANGER_TOKEN}"))
                .body(Body::empty())
                .unwrap();
            let at = Instant::now();
            let resp = router.clone().oneshot(req).await.unwrap();
            log.lock().unwrap().push((at, held));
            if resp.status() == StatusCode::OK {
                let bytes = resp.into_body().collect().await.unwrap().to_bytes();
                let body: Value = serde_json::from_slice(&bytes).unwrap();
                held = body[version_field].as_i64().expect("version");
            }
            tokio::time::sleep(poll).await;
        }
    });
    Plugin {
        id: plugin_id,
        polls: polls.clone(),
        policy_polls: polls,
        task,
        others: Vec::new(),
    }
}

/// A running fake plugin; dropping it stops it. Each poll is recorded as
/// (when, the `lastKnownVersion` it reported).
pub struct Plugin {
    pub id: String,
    pub polls: Arc<Mutex<Vec<(Instant, i64)>>>,
    /// The policy polls: for Trino its second poller's, for StarRocks the same
    /// log as `polls`.
    pub policy_polls: Arc<Mutex<Vec<(Instant, i64)>>>,
    task: tokio::task::JoinHandle<()>,
    /// Pollers stopped with this one.
    others: Vec<Plugin>,
}

impl Plugin {
    /// The first poll that reported at least `version`.
    pub fn first_report_of(&self, version: i64) -> Option<Instant> {
        self.polls
            .lock()
            .unwrap()
            .iter()
            .find(|(_, held)| *held >= version)
            .map(|(t, _)| *t)
    }

    /// The first poll that reported at least `version`, waiting up to 5 s for
    /// it: a commit that did not wait reaches lakekeeper before it exists.
    pub async fn report_of(&self, version: i64) -> Instant {
        for _ in 0..250 {
            if let Some(t) = self.first_report_of(version) {
                return t;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the plugin never reported version {version}");
    }

    /// The first policy poll that reported at least `version`, waiting up to
    /// 5 s for it.
    pub async fn policy_report_of(&self, version: i64) -> Instant {
        for _ in 0..250 {
            let seen = self
                .policy_polls
                .lock()
                .unwrap()
                .iter()
                .find(|(_, held)| *held >= version)
                .map(|(t, _)| *t);
            if let Some(t) = seen {
                return t;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the plugin never reported policy version {version}");
    }

    /// Wait until the plugin has polled once, so the ledger knows it.
    pub async fn polled(&self) {
        for _ in 0..100 {
            if !self.polls.lock().unwrap().is_empty()
                && !self.policy_polls.lock().unwrap().is_empty()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the fake plugin never polled");
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        self.task.abort();
    }
}
