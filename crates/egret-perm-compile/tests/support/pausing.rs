//! A `GroupMembershipRecompiler` that delegates to the real
//! `PermGroupRecompiler` but STOPS at one chosen point of a membership write
//! until the test lets it go — the explicit step that turns a concurrency race
//! into a deterministic interleaving (J-F3 Task 1b, fix round 1).
//!
//! The membership write runs on its own transaction (its own pooled
//! connection), so while it is parked here it holds every lock it has taken so
//! far: the advisory user and group locks, and — at [`Stage::BeforeRecompile`]
//! — its profile locks and its uncommitted `user_groups` change. The test then
//! drives the competing write and observes whether it is made to wait.

// Each test binary that includes this uses a different subset of it.
#![allow(dead_code)]

use std::sync::Mutex;

use egret_auth::groups::GroupMembershipRecompiler;
use egret_perm_compile::PermGroupRecompiler;
use sqlx::{Postgres, Transaction};
use tokio::sync::oneshot;

/// Where the write parks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Inside `lock_affected_in_tx`, AFTER the affected set was computed and
    /// its profiles locked — the affected set is now fixed.
    AfterAffected,
    /// Inside `recompile_in_tx`, before the recompile — the membership row has
    /// been written, not committed.
    BeforeRecompile,
}

pub struct Pausing {
    stage: Stage,
    reached: Mutex<Option<oneshot::Sender<()>>>,
    go: tokio::sync::Mutex<Option<oneshot::Receiver<()>>>,
}

/// The two ends the test holds: wait on `reached`, then send on `go`.
pub struct Handle {
    pub reached: oneshot::Receiver<()>,
    pub go: oneshot::Sender<()>,
}

impl Pausing {
    pub fn new(stage: Stage) -> (Self, Handle) {
        let (reached_tx, reached_rx) = oneshot::channel();
        let (go_tx, go_rx) = oneshot::channel();
        (
            Self {
                stage,
                reached: Mutex::new(Some(reached_tx)),
                go: tokio::sync::Mutex::new(Some(go_rx)),
            },
            Handle {
                reached: reached_rx,
                go: go_tx,
            },
        )
    }

    async fn park(&self, here: Stage) {
        if here != self.stage {
            return;
        }
        if let Some(tx) = self.reached.lock().unwrap().take() {
            let _ = tx.send(());
        }
        if let Some(rx) = self.go.lock().await.take() {
            let _ = rx.await;
        }
    }
}

#[async_trait::async_trait]
impl GroupMembershipRecompiler for Pausing {
    async fn lock_affected_in_tx(
        &self,
        tx: &mut Transaction<'static, Postgres>,
        group_names: &[String],
    ) -> egret_core::Result<Vec<i64>> {
        let out = PermGroupRecompiler
            .lock_affected_in_tx(tx, group_names)
            .await?;
        self.park(Stage::AfterAffected).await;
        Ok(out)
    }

    async fn recompile_in_tx(
        &self,
        tx: &mut Transaction<'static, Postgres>,
        service_ids: &[i64],
    ) -> egret_core::Result<()> {
        self.park(Stage::BeforeRecompile).await;
        PermGroupRecompiler.recompile_in_tx(tx, service_ids).await
    }
}
