//! W9c Task 9: **the one loop that owns a transaction whose renders read
//! through a [`Prefetch`]** (`storage::prefetch`, the user's ruling of
//! 2026-10-08).
//!
//! Every outermost owner of a transaction that reaches `settle` runs its body
//! in [`transaction!`]: `run_one` (a finished task's outcome),
//! [`super::resume`], [`super::start_execution_with_trigger`], the start and
//! replay routes, the scheduler's fire and the Flow-trigger processor's
//! event. Each attempt:
//!
//! 1. begins a transaction ([`begin`]) and runs the body with this call's
//!    cache;
//! 2. **if a render recorded a miss, rolls back** ([`finish`]) -- whatever the
//!    body answered, `Ok` or `Err`, because a body's answer is not trusted
//!    while one of its renders failed for want of a value -- then runs the
//!    fetch with no transaction open and no lock held, and runs the body
//!    again;
//! 3. otherwise commits (or rolls back) as the body says.
//!
//! So **no attempt commits with a miss pending**. [`finish`]'s check is the
//! gate; `settle`'s early `Err(PrefetchMiss)` only saves the work an attempt
//! that will be rolled back would do. The rounds and the cache's size are
//! bounded by the cache itself (`storage::prefetch::MAX_ROUNDS`,
//! `CACHE_CAP_FACTOR`).
//!
//! # Why a macro
//!
//! The body borrows the owner's locals and the attempt's connection, and the
//! future it makes must be `Send` (the claim loop and the routes are spawned
//! tasks). An `AsyncFnMut` parameter cannot say that today (its future's
//! `Send` is checked for every lifetime and fails, rust-lang/rust#110338),
//! so the loop is written once here as a macro and the body is an inline
//! `async` block. The decisions -- how an attempt begins, and the gate --
//! are the two functions below, not macro text.

use egret_core::{Error, Result};
use sqlx::{PgPool, Postgres, Transaction};

use crate::storage::prefetch::Prefetch;

/// Keeps the `sqlx::Error` as the source, as `claim`'s does.
fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::Error::new(e))
}

/// What the body asks of its transaction.
pub(crate) enum Finish<T> {
    Commit(T),
    /// Roll back and answer `T`: nothing to keep (no work was claimed, a
    /// claim met a serialization failure).
    Rollback(T),
}

/// What a fetch phase came to.
pub(crate) enum Fetched {
    /// The misses are answered: run the body again.
    Again,
    /// The owner gave its work up during the fetch (a claimed run's lease was
    /// lost): stop, committing nothing.
    GiveUp,
}

/// How a [`transaction!`] ended without an error.
pub(crate) enum Ended<T> {
    Done(T),
    GaveUp,
}

/// What [`finish`] decided.
pub(crate) enum Next<T> {
    /// A miss is pending: the attempt is rolled back; fetch, then again.
    Fetch,
    Done(T),
}

/// The isolation an attempt begins with.
#[derive(Clone, Copy)]
pub(crate) enum Isolation {
    /// The pool's default, `READ COMMITTED`.
    Default,
    /// `REPEATABLE READ`: the Flow-trigger processor's event transaction.
    RepeatableRead,
}

/// Begin one attempt.
pub(crate) async fn begin(
    pool: &PgPool,
    isolation: Isolation,
) -> Result<Transaction<'static, Postgres>> {
    match isolation {
        Isolation::Default => pool.begin().await,
        Isolation::RepeatableRead => {
            pool.begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ")
                .await
        }
    }
    .map_err(db_err)
}

/// **THE gate.** An attempt that recorded a miss is rolled back, whatever it
/// answered, and never committed; any other is committed or rolled back as
/// its body said, and an `Err` rolls back.
pub(crate) async fn finish<T>(
    tx: Transaction<'static, Postgres>,
    pf: &Prefetch,
    answered: Result<Finish<T>>,
) -> Result<Next<T>> {
    if pf.has_pending() {
        // Best effort: dropping the transaction rolls it back too.
        let _ = tx.rollback().await;
        return Ok(Next::Fetch);
    }
    match answered {
        Ok(Finish::Commit(v)) => {
            tx.commit().await.map_err(db_err)?;
            Ok(Next::Done(v))
        }
        Ok(Finish::Rollback(v)) => {
            tx.rollback().await.map_err(db_err)?;
            Ok(Next::Done(v))
        }
        Err(e) => {
            let _ = tx.rollback().await;
            Err(e)
        }
    }
}

/// The loop (see the module doc).
///
/// * `transaction!(pool, isolation, |conn, pf| { body }, |pf| { fetch })`:
///   `body` is the attempt, evaluating to `Result<Finish<T>>` (`?` inside it
///   ends the ATTEMPT, never the loop, so it cannot skip the gate); `fetch`
///   runs after the rollback and evaluates to `Result<Fetched>`. The whole
///   evaluates to `Result<Ended<T>>`.
/// * `transaction!(pool, |conn, pf| { body })`: `body` evaluates to
///   `Result<T>` and commits on `Ok`; the fetch is the plain
///   `pf.fetch_pending()`. The whole evaluates to `Result<T>`.
///
/// `conn` is `&mut PgConnection`, `pf` is `&Prefetch`; a fresh `Prefetch`
/// per use, so the cache lives for this call only.
macro_rules! transaction {
    ($pool:expr, $isolation:expr, |$conn:ident, $pf:ident| $body:block, |$fpf:ident| $fetch:block) => {{
        let ended: egret_core::Result<$crate::worker::txn::Ended<_>> = async {
            let cache = $crate::storage::prefetch::Prefetch::new();
            loop {
                let mut tx = $crate::worker::txn::begin($pool, $isolation).await?;
                let answered = {
                    let $conn: &mut sqlx::PgConnection = &mut *tx;
                    let $pf: &$crate::storage::prefetch::Prefetch = &cache;
                    async { $body }.await
                };
                match $crate::worker::txn::finish(tx, &cache, answered).await? {
                    $crate::worker::txn::Next::Done(v) => {
                        return Ok($crate::worker::txn::Ended::Done(v))
                    }
                    $crate::worker::txn::Next::Fetch => {
                        let $fpf: &$crate::storage::prefetch::Prefetch = &cache;
                        let fetched: egret_core::Result<$crate::worker::txn::Fetched> =
                            async { $fetch }.await;
                        match fetched? {
                            $crate::worker::txn::Fetched::Again => continue,
                            $crate::worker::txn::Fetched::GiveUp => {
                                return Ok($crate::worker::txn::Ended::GaveUp)
                            }
                        }
                    }
                }
            }
        }
        .await;
        ended
    }};
    ($pool:expr, |$conn:ident, $pf:ident| $body:block) => {{
        let ended = $crate::worker::txn::transaction!(
            $pool,
            $crate::worker::txn::Isolation::Default,
            |$conn, $pf| {
                let answered: egret_core::Result<_> = async { $body }.await;
                answered.map($crate::worker::txn::Finish::Commit)
            },
            |fetch_pf| {
                fetch_pf
                    .fetch_pending()
                    .await
                    .map(|()| $crate::worker::txn::Fetched::Again)
            }
        );
        match ended {
            Ok($crate::worker::txn::Ended::Done(v)) => Ok(v),
            Ok($crate::worker::txn::Ended::GaveUp) => Err(egret_core::Error::Internal(
                anyhow::anyhow!("a transaction's plain fetch cannot give up"),
            )),
            Err(e) => Err(e),
        }
    }};
}
pub(crate) use transaction;
