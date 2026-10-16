//! The `WorkingDirectory` block's own work, and its children's snapshots
//! (spec §4.3, §5.1–§5.4).
//!
//! A block has two claims that no handler runs: **prepare** (`PREPARING`,
//! before the first child) and **finish** (`FINISHING`, after the last one,
//! whatever a child did). The worker completes them in
//! `claim::run_block_claim`, through the same lease, fence and settle a leaf
//! uses, and the functions here are the work inside that lease.
//!
//! Every failure here is the block's: [`run_claim`] returns
//! [`BlockDone::Failed`] rather than an `Err`, so a block never sits in
//! `PREPARING` for want of a completed claim (V3 and the no-silent-stall rule).

use super::claim::Claimed;
use super::WorkingDirCarry;

/// The object a block's prepare claim writes: the empty directory the first
/// child starts from. Its name is under the block's prefix, so the finish
/// claim's discard removes it with the children's snapshots.
const PREPARE_OBJECT: &str = "prepare.tar";

/// What a block's own claim produced, for `claim::run_block_claim` to commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BlockDone {
    /// The prepare claim is done; `snapshot` is the pointer the first child
    /// starts from.
    Prepared { snapshot: String },
    /// The finish claim is done. Its work (`outputFiles`, cache) is Task 7's.
    Finished,
    /// The claim could not be completed; the block fails with this reason.
    Failed(String),
}

/// The carry of a child of a `WorkingDirectory` block, or `Err` with the reason
/// it cannot start: a block that has run no prepare claim has no snapshot, and
/// a child on an empty directory would be a silent wrong answer (spec §5.2).
pub(crate) fn carry_of(c: &Claimed) -> Result<Option<WorkingDirCarry>, String> {
    let Some(block) = c.block_id else {
        return Ok(None);
    };
    let Some(start_from) = c.block_snapshot.clone() else {
        return Err(format!(
            "corrupt: the WorkingDirectory block of {} has no snapshot to start it from",
            c.task_id
        ));
    };
    let Some(store) = crate::storage::current() else {
        return Err(format!(
            "{} is a child of a WorkingDirectory, which needs internal storage for its \
             snapshots, and none is installed",
            c.task_id
        ));
    };
    Ok(Some(WorkingDirCarry {
        start_from,
        carry_to: store.working_dir_uri(
            c.workspace_id,
            c.execution_id,
            block,
            &format!("{}.tar", c.id),
        ),
        cap: store.caps.max_task_bytes,
    }))
}

/// Do the block's own claim named by `c.block_phase`. Never `Err`: whatever
/// goes wrong, the block is failed, and the failure is committed like any
/// outcome.
pub(crate) async fn run_claim(c: &Claimed) -> BlockDone {
    let Some(store) = crate::storage::current() else {
        return BlockDone::Failed(format!(
            "the WorkingDirectory {} needs internal storage for its snapshots, and none is installed",
            c.task_id
        ));
    };
    match c.block_phase.as_deref() {
        Some("PREPARING") => {
            let uri = store.working_dir_uri(c.workspace_id, c.execution_id, c.id, PREPARE_OBJECT);
            // An archive of nothing: a directory with no entries, which the
            // first child starts from. 1024 zero bytes is the ustar end marker.
            match store.store.put(&uri, vec![0u8; 1024]).await {
                Ok(()) => BlockDone::Prepared { snapshot: uri },
                Err(e) => BlockDone::Failed(format!(
                    "the WorkingDirectory {} could not write its snapshot: {e}",
                    c.task_id
                )),
            }
        }
        Some("FINISHING") => BlockDone::Finished,
        other => BlockDone::Failed(format!(
            "the WorkingDirectory {} was claimed in {other:?}, which is not a phase it runs",
            c.task_id
        )),
    }
}

/// Remove every snapshot of block run `c.id`, after its finish claim has
/// committed. Best effort: an object left behind is the pruner's to sweep
/// (spec §7.2), and the block is already finished.
pub(crate) async fn discard(c: &Claimed) {
    let Some(store) = crate::storage::current() else {
        return;
    };
    let prefix = store.working_dir_prefix(c.workspace_id, c.execution_id, c.id);
    if let Err(e) = store.store.delete_prefix(&prefix).await {
        tracing::warn!(
            execution = %c.execution_id,
            run = %c.id,
            "the snapshots of a finished WorkingDirectory could not be removed (the pruner will \
             sweep them): {e}"
        );
    }
}
