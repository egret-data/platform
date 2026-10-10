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
use egret_core::{Result, UserId, WorkspaceId};
use uuid::Uuid;

use crate::proxy::pod_ref::{edit_pod_ref, run_pod_ref};
use crate::proxy::{PodEndpoint, PodResolver};

/// One edit pod per author per workspace (D8): every kernel in it runs as that one person.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EditPodKey {
    pub user_id: UserId,
    pub workspace_id: WorkspaceId,
}

impl EditPodKey {
    /// The URL segment and pod-name stem; the same function the proxy checks every request with.
    pub fn pod_ref(&self) -> String {
        edit_pod_ref(self.user_id, self.workspace_id)
    }
}

/// One run pod per (viewer, published version) (D24): a run pod never holds a second viewer's
/// kernel, because marimo's IPC channels have no authentication of their own (see the plan this
/// decision came from) and run pods default to marimo's own thread-mode kernel, which is safe
/// only because every session inside one pod already belongs to the one viewer it was opened for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RunPodKey {
    pub user_id: UserId,
    pub version_id: Uuid,
}

impl RunPodKey {
    pub fn pod_ref(&self) -> String {
        run_pod_ref(self.user_id, self.version_id)
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

    /// The viewer's own run pod for this version: reuse a live one, otherwise start it and wait
    /// until it answers. Calling this IS the signal that the pod is in use.
    async fn ensure_run(&self, key: RunPodKey, timeout: Duration) -> Result<PodEndpoint>;

    /// Stop the pod. A pod that is already gone is not an error.
    async fn stop_run(&self, key: RunPodKey) -> Result<()>;

    /// Run pods nobody has asked for in `idle`; the reaper stops them.
    async fn idle_run_pods(&self, idle: Duration) -> Result<Vec<RunPodKey>>;

    fn mode(&self) -> &'static str;
}

/// A shared launcher is a launcher: the proxy's resolver and the reaper hold the same one.
#[async_trait]
impl<T: AppPodLauncher + ?Sized> AppPodLauncher for std::sync::Arc<T> {
    async fn ensure_edit(&self, key: EditPodKey, timeout: Duration) -> Result<PodEndpoint> {
        (**self).ensure_edit(key, timeout).await
    }
    async fn stop_edit(&self, key: EditPodKey) -> Result<()> {
        (**self).stop_edit(key).await
    }
    async fn idle_edit_pods(&self, idle: Duration) -> Result<Vec<EditPodKey>> {
        (**self).idle_edit_pods(idle).await
    }
    async fn ensure_run(&self, key: RunPodKey, timeout: Duration) -> Result<PodEndpoint> {
        (**self).ensure_run(key, timeout).await
    }
    async fn stop_run(&self, key: RunPodKey) -> Result<()> {
        (**self).stop_run(key).await
    }
    async fn idle_run_pods(&self, idle: Duration) -> Result<Vec<RunPodKey>> {
        (**self).idle_run_pods(idle).await
    }
    fn mode(&self) -> &'static str {
        (**self).mode()
    }
}

/// Every `every`, stop the edit pods idle `edit_idle` and the run pods idle `run_idle` (the two
/// differ — D24's run_idle_secs defaults far shorter than an author's edit_idle). Runs until the
/// runtime ends. A failure to stop one pod, of either kind, is logged and the next tick tries
/// again; one kind's failure never skips the other's tick.
pub fn spawn_pod_reaper(
    launcher: std::sync::Arc<dyn AppPodLauncher>,
    edit_idle: Duration,
    run_idle: Duration,
    every: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        loop {
            tick.tick().await;
            match launcher.idle_edit_pods(edit_idle).await {
                Ok(keys) => {
                    for key in keys {
                        match launcher.stop_edit(key).await {
                            Ok(()) => {
                                tracing::info!(pod = %key.pod_ref(), "stopped an idle app pod")
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, pod = %key.pod_ref(), "app pod reaper: stop failed")
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "app pod reaper: listing idle edit pods failed")
                }
            }
            match launcher.idle_run_pods(run_idle).await {
                Ok(keys) => {
                    for key in keys {
                        match launcher.stop_run(key).await {
                            Ok(()) => {
                                tracing::info!(pod = %key.pod_ref(), "stopped an idle app run pod")
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, pod = %key.pod_ref(), "app run pod reaper: stop failed")
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "app pod reaper: listing idle run pods failed")
                }
            }
        }
    })
}

/// Makes any launcher the proxy's [`PodResolver`]. Run pods arrive with P5.
pub struct LauncherPods<L> {
    pub launcher: L,
    pub ready_timeout: Duration,
}

#[async_trait]
impl<L: AppPodLauncher> PodResolver for LauncherPods<L> {
    fn edit_ref(&self, user: UserId, ws: WorkspaceId) -> String {
        edit_pod_ref(user, ws)
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

    fn run_ref(&self, user: UserId, version: Uuid) -> String {
        run_pod_ref(user, version)
    }

    async fn run_pod(&self, user: UserId, _app: Uuid, version: Uuid) -> Result<PodEndpoint> {
        self.launcher
            .ensure_run(
                RunPodKey {
                    user_id: user,
                    version_id: version,
                },
                self.ready_timeout,
            )
            .await
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
    live_run: HashMap<RunPodKey, u32>,
    started_run: Vec<RunPodKey>,
    stopped_run: Vec<RunPodKey>,
    idle_run: Vec<RunPodKey>,
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

    /// Run pods started so far (a reused pod is not started again).
    pub fn started_run(&self) -> Vec<RunPodKey> {
        self.state.lock().unwrap().started_run.clone()
    }

    pub fn stopped_run(&self) -> Vec<RunPodKey> {
        self.state.lock().unwrap().stopped_run.clone()
    }

    /// What `idle_run_pods` will report next.
    pub fn set_idle_run(&self, keys: Vec<RunPodKey>) {
        self.state.lock().unwrap().idle_run = keys;
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

    async fn ensure_run(&self, key: RunPodKey, _timeout: Duration) -> Result<PodEndpoint> {
        let mut st = self.state.lock().unwrap();
        let uses = st.live_run.entry(key).or_insert(0);
        *uses += 1;
        if *uses == 1 {
            st.started_run.push(key);
        }
        Ok(self.endpoint.clone())
    }

    async fn stop_run(&self, key: RunPodKey) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        st.live_run.remove(&key);
        st.stopped_run.push(key);
        Ok(())
    }

    async fn idle_run_pods(&self, _idle: Duration) -> Result<Vec<RunPodKey>> {
        Ok(std::mem::take(&mut self.state.lock().unwrap().idle_run))
    }

    fn mode(&self) -> &'static str {
        "fake"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_core::Error;

    fn key() -> EditPodKey {
        EditPodKey {
            user_id: UserId(Uuid::new_v4()),
            workspace_id: WorkspaceId(Uuid::new_v4()),
        }
    }

    fn run_key() -> RunPodKey {
        RunPodKey {
            user_id: UserId(Uuid::new_v4()),
            version_id: Uuid::new_v4(),
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
    async fn run_pods_are_tracked_independently_of_edit_pods() {
        let l = fake();
        let (edit_key, run_key_a, run_key_b) = (key(), run_key(), run_key());
        l.ensure_edit(edit_key, Duration::from_secs(1))
            .await
            .unwrap();
        l.ensure_run(run_key_a, Duration::from_secs(1))
            .await
            .unwrap();
        l.ensure_run(run_key_a, Duration::from_secs(1))
            .await
            .unwrap(); // reused, not started again
        l.ensure_run(run_key_b, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(
            l.started(),
            [edit_key],
            "the run pods must not show up as edit pods"
        );
        assert_eq!(l.started_run(), [run_key_a, run_key_b]);
        l.stop_run(run_key_a).await.unwrap();
        assert_eq!(l.stopped_run(), [run_key_a]);
        assert!(
            l.stopped().is_empty(),
            "stopping a run pod must not touch the edit list"
        );
    }

    #[tokio::test]
    async fn the_run_idle_list_is_handed_over_once_and_separately_from_edit() {
        let l = fake();
        let rk = run_key();
        l.set_idle_run(vec![rk]);
        assert_eq!(l.idle_run_pods(Duration::from_secs(1)).await.unwrap(), [rk]);
        assert!(l
            .idle_run_pods(Duration::from_secs(1))
            .await
            .unwrap()
            .is_empty());
        assert!(l
            .idle_edit_pods(Duration::from_secs(1))
            .await
            .unwrap()
            .is_empty());
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
    }

    #[tokio::test]
    async fn the_resolver_also_starts_the_viewers_own_run_pod_by_version() {
        let (u, v) = (UserId(Uuid::new_v4()), Uuid::new_v4());
        let pods = LauncherPods {
            launcher: fake(),
            ready_timeout: Duration::from_secs(3),
        };
        assert_eq!(
            pods.run_ref(u, v),
            RunPodKey {
                user_id: u,
                version_id: v
            }
            .pod_ref()
        );
        let ep = pods.run_pod(u, Uuid::new_v4(), v).await.unwrap();
        assert_eq!(ep.base.as_str(), "http://10.0.0.1:2718/");
        assert_eq!(
            pods.launcher.started_run(),
            [RunPodKey {
                user_id: u,
                version_id: v
            }]
        );
    }

    #[tokio::test]
    async fn the_reaper_stops_idle_pods_and_survives_a_failing_one() {
        use std::sync::Arc;
        struct Flaky {
            inner: FakeAppPodLauncher,
            fail_first: Mutex<Option<EditPodKey>>,
        }
        #[async_trait]
        impl AppPodLauncher for Flaky {
            async fn ensure_edit(&self, k: EditPodKey, t: Duration) -> Result<PodEndpoint> {
                self.inner.ensure_edit(k, t).await
            }
            async fn stop_edit(&self, k: EditPodKey) -> Result<()> {
                if self.fail_first.lock().unwrap().take() == Some(k) {
                    return Err(Error::from(anyhow::anyhow!("cluster hiccup")));
                }
                self.inner.stop_edit(k).await
            }
            async fn idle_edit_pods(&self, i: Duration) -> Result<Vec<EditPodKey>> {
                self.inner.idle_edit_pods(i).await
            }
            async fn ensure_run(&self, k: RunPodKey, t: Duration) -> Result<PodEndpoint> {
                self.inner.ensure_run(k, t).await
            }
            async fn stop_run(&self, k: RunPodKey) -> Result<()> {
                self.inner.stop_run(k).await
            }
            async fn idle_run_pods(&self, i: Duration) -> Result<Vec<RunPodKey>> {
                self.inner.idle_run_pods(i).await
            }
            fn mode(&self) -> &'static str {
                "flaky"
            }
        }
        let (a, b) = (key(), key());
        let r = run_key();
        let launcher = Arc::new(Flaky {
            inner: fake(),
            fail_first: Mutex::new(Some(a)),
        });
        launcher.inner.set_idle(vec![a, b]);
        launcher.inner.set_idle_run(vec![r]);
        let handle = spawn_pod_reaper(
            launcher.clone(),
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_millis(20),
        );
        // `a` failed once and stays idle: report it again; `b` was stopped on the first tick.
        // `r` is a run pod, reaped on the same tick but tracked on its own list entirely.
        for _ in 0..100 {
            if launcher.inner.stopped() == [b] && launcher.inner.stopped_run() == [r] {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            launcher.inner.stopped(),
            [b],
            "one failure must not stop the others"
        );
        assert_eq!(
            launcher.inner.stopped_run(),
            [r],
            "the run pod is reaped on its own tick, independent of the edit failure"
        );
        launcher.inner.set_idle(vec![a]);
        for _ in 0..100 {
            if launcher.inner.stopped().contains(&a) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            launcher.inner.stopped(),
            [b, a],
            "the loop is still alive and retried `a`"
        );
        handle.abort();
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
