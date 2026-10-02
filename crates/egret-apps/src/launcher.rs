//! Starting and stopping the pods that run marimo (spec §6.3).
//!
//! This is a trait of its own, in this crate, rather than a widening of `egret-kernel`'s
//! `KernelLauncher` (D22): the Jupyter half of `egret-kernel` is deleted in P6, and a pod here
//! is keyed, credentialed and reaped differently — keyed by (author, workspace), given a pod
//! token and a marimo token it never prints, and stopped when nobody has used it for a while.
//!
//! A launcher MINTS the pod token when it creates a pod and REVOKES it when it stops one, so a
//! copy of a dead pod's token is worth nothing. The marimo token never touches the database:
//! the proxy asks the launcher for it on every request, and a launcher that restarted reads it
//! back from wherever it put it (a Secret, a process table).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use egret_core::{Error, Result, UserId, WorkspaceId};
use uuid::Uuid;

use crate::proxy::{pod_ref, PodEndpoint, PodResolver};

/// One edit pod per author per workspace (D8): every kernel in it runs as that one person.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EditPodKey {
    pub user_id: UserId,
    pub workspace_id: WorkspaceId,
}

impl EditPodKey {
    /// The URL segment and pod-name stem; the same function the proxy checks every request with.
    pub fn pod_ref(&self) -> String {
        pod_ref(self.user_id, self.workspace_id)
    }
}

#[async_trait]
pub trait AppPodLauncher: Send + Sync {
    /// The author's edit pod: reuse a live one, otherwise start it and wait until it answers,
    /// at most `timeout`. Calling this IS the signal that the pod is in use — it is what the
    /// idle reaper measures.
    async fn ensure_edit(&self, key: EditPodKey, timeout: Duration) -> Result<PodEndpoint>;

    /// Stop the pod and revoke its pod token. A pod that is already gone is not an error.
    async fn stop_edit(&self, key: EditPodKey) -> Result<()>;

    /// Edit pods nobody has asked for in `idle`; the reaper stops them.
    async fn idle_edit_pods(&self, idle: Duration) -> Result<Vec<EditPodKey>>;

    fn mode(&self) -> &'static str;
}

/// Makes any launcher the proxy's [`PodResolver`]. Run pods arrive with P5.
pub struct LauncherPods<L> {
    pub launcher: L,
    pub ready_timeout: Duration,
}

#[async_trait]
impl<L: AppPodLauncher> PodResolver for LauncherPods<L> {
    fn edit_ref(&self, user: UserId, ws: WorkspaceId) -> String {
        pod_ref(user, ws)
    }

    async fn edit_pod(&self, user: UserId, ws: WorkspaceId) -> Result<PodEndpoint> {
        self.launcher
            .ensure_edit(
                EditPodKey {
                    user_id: user,
                    workspace_id: ws,
                },
                self.ready_timeout,
            )
            .await
    }

    async fn run_pod(&self, _app: Uuid, _version: Uuid) -> Result<PodEndpoint> {
        Err(Error::NotFound("run pod".into()))
    }
}

/// A launcher with no processes: records what it was asked and hands back one endpoint.
pub struct FakeAppPodLauncher {
    pub endpoint: PodEndpoint,
    state: Mutex<FakeState>,
}

#[derive(Default)]
struct FakeState {
    live: HashMap<EditPodKey, u32>,
    started: Vec<EditPodKey>,
    stopped: Vec<EditPodKey>,
    idle: Vec<EditPodKey>,
}

impl FakeAppPodLauncher {
    pub fn new(endpoint: PodEndpoint) -> Self {
        Self {
            endpoint,
            state: Mutex::default(),
        }
    }

    /// Pods started so far (a reused pod is not started again).
    pub fn started(&self) -> Vec<EditPodKey> {
        self.state.lock().unwrap().started.clone()
    }

    pub fn stopped(&self) -> Vec<EditPodKey> {
        self.state.lock().unwrap().stopped.clone()
    }

    /// What `idle_edit_pods` will report next.
    pub fn set_idle(&self, keys: Vec<EditPodKey>) {
        self.state.lock().unwrap().idle = keys;
    }
}

#[async_trait]
impl AppPodLauncher for FakeAppPodLauncher {
    async fn ensure_edit(&self, key: EditPodKey, _timeout: Duration) -> Result<PodEndpoint> {
        let mut st = self.state.lock().unwrap();
        let uses = st.live.entry(key).or_insert(0);
        *uses += 1;
        if *uses == 1 {
            st.started.push(key);
        }
        Ok(self.endpoint.clone())
    }

    async fn stop_edit(&self, key: EditPodKey) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        st.live.remove(&key);
        st.stopped.push(key);
        Ok(())
    }

    async fn idle_edit_pods(&self, _idle: Duration) -> Result<Vec<EditPodKey>> {
        Ok(std::mem::take(&mut self.state.lock().unwrap().idle))
    }

    fn mode(&self) -> &'static str {
        "fake"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> EditPodKey {
        EditPodKey {
            user_id: UserId(Uuid::new_v4()),
            workspace_id: WorkspaceId(Uuid::new_v4()),
        }
    }

    fn fake() -> FakeAppPodLauncher {
        FakeAppPodLauncher::new(PodEndpoint {
            base: url::Url::parse("http://10.0.0.1:2718").unwrap(),
            marimo_token: "m".into(),
        })
    }

    #[tokio::test]
    async fn one_pod_per_key_and_a_second_request_reuses_it() {
        let l = fake();
        let (a, b) = (key(), key());
        l.ensure_edit(a, Duration::from_secs(1)).await.unwrap();
        l.ensure_edit(a, Duration::from_secs(1)).await.unwrap();
        l.ensure_edit(b, Duration::from_secs(1)).await.unwrap();
        assert_eq!(l.started(), [a, b]);
        l.stop_edit(a).await.unwrap();
        l.ensure_edit(a, Duration::from_secs(1)).await.unwrap();
        assert_eq!(l.started(), [a, b, a], "a stopped pod is started afresh");
    }

    #[tokio::test]
    async fn the_resolver_starts_the_callers_own_pod_and_names_it_with_the_shared_ref() {
        let k = key();
        let pods = LauncherPods {
            launcher: fake(),
            ready_timeout: Duration::from_secs(3),
        };
        assert_eq!(pods.edit_ref(k.user_id, k.workspace_id), k.pod_ref());
        let ep = pods.edit_pod(k.user_id, k.workspace_id).await.unwrap();
        assert_eq!(ep.base.as_str(), "http://10.0.0.1:2718/");
        assert_eq!(pods.launcher.started(), [k]);
        assert!(matches!(
            pods.run_pod(Uuid::new_v4(), Uuid::new_v4()).await,
            Err(Error::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn the_idle_list_is_handed_over_once() {
        let l = fake();
        let k = key();
        l.set_idle(vec![k]);
        assert_eq!(l.idle_edit_pods(Duration::from_secs(1)).await.unwrap(), [k]);
        assert!(l
            .idle_edit_pods(Duration::from_secs(1))
            .await
            .unwrap()
            .is_empty());
    }
}
