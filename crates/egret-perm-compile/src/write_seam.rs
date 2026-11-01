//! The pause point between a grant or rule PATCH's first read of the stored
//! row and its lock wait (#789).
//!
//! A PATCH reads the stored row before it takes any lock (it needs the row to
//! know which group locks to take), so a concurrent PATCH of the same row can
//! commit in between. What the write does about that is only testable with a
//! deterministic interleaving, and the two steps sit in one function with no
//! other hook. So the write calls [`before_locks`] there; a test runs the
//! request inside [`run_with`] and holds it at that point until it has
//! committed the competing write.
//!
//! Inert in production: the point is a task-local nobody scopes outside a
//! test, so [`before_locks`] finds none and returns at once.

use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

/// One pause: the write signals `reached` when it gets there, then waits for
/// `go`. Used once; a second write in the same scope passes straight through.
pub struct PausePoint {
    reached: Mutex<Option<oneshot::Sender<()>>>,
    go: tokio::sync::Mutex<Option<oneshot::Receiver<()>>>,
}

/// The test's ends of a [`PausePoint`]: await `reached`, then send on `go`.
pub struct PauseHandle {
    pub reached: oneshot::Receiver<()>,
    pub go: oneshot::Sender<()>,
}

impl PausePoint {
    pub fn new() -> (Arc<Self>, PauseHandle) {
        let (reached_tx, reached_rx) = oneshot::channel();
        let (go_tx, go_rx) = oneshot::channel();
        (
            Arc::new(Self {
                reached: Mutex::new(Some(reached_tx)),
                go: tokio::sync::Mutex::new(Some(go_rx)),
            }),
            PauseHandle {
                reached: reached_rx,
                go: go_tx,
            },
        )
    }
}

tokio::task_local! {
    static BEFORE_LOCKS: Arc<PausePoint>;
}

/// Run `f` (a request through the router, say) with `point` set, so the PATCH
/// it makes stops at [`before_locks`].
pub async fn run_with<F: Future>(point: Arc<PausePoint>, f: F) -> F::Output {
    BEFORE_LOCKS.scope(point, f).await
}

/// Called by the PATCH path after its first read of the stored row and before
/// its first lock. Returns at once unless a test scoped a [`PausePoint`].
pub(crate) async fn before_locks() {
    let Ok(point) = BEFORE_LOCKS.try_with(Arc::clone) else {
        return;
    };
    let reached = point.reached.lock().expect("pause point").take();
    if let Some(tx) = reached {
        let _ = tx.send(());
    }
    let go = point.go.lock().await.take();
    if let Some(rx) = go {
        let _ = rx.await;
    }
}
