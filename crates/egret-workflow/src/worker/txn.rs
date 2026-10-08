//! W9c Task 9: **the one loop that owns a transaction whose renders read
//! through a [`Prefetch`]** (`storage::prefetch`, the user's ruling of
//! 2026-10-08).
//!
//! Every outermost owner of a transaction that reaches `settle` runs its body
//! in [`transaction!`]: `run_one` (a finished task's outcome),
//! [`super::resume`], [`super::start_execution_with_trigger`],
//! [`super::commit_outcome`], the start and replay routes, the scheduler's
//! fire and the Flow-trigger processor's event. Each attempt:
//!
//! 1. begins a transaction ([`Txn::begin`]) and runs the body with this
//!    call's cache;
//! 2. **if a render recorded a miss, rolls back** ([`finish`]) -- whatever the
//!    body answered, `Ok` or `Err`, because a body that rendered a placeholder
//!    computed from a value it did not have: its answer, its errors and its
//!    writes are all discarded -- then runs the fetch with no transaction
//!    open and no lock held, and runs the body again;
//! 3. otherwise commits (or rolls back) as the body says.
//!
//! So **no attempt commits with a miss pending**, and [`finish`]'s check is
//! the ONLY miss check: a render that misses carries on with a placeholder
//! (the user's ruling of 2026-10-08, "gather"), so one attempt discovers
//! every read it can reach. The rounds, the fetches and the cache's size are
//! bounded by the cache itself (`storage::prefetch::MAX_ROUNDS`,
//! `MAX_FETCHES`, `CACHE_CAP_FACTOR`).
//!
//! # Why a commit cannot skip the gate (final review m2)
//!
//! * An [`Txn`] OWNS its transaction, in a private field: nothing but
//!   [`finish`] can commit it, and dropping it rolls it back.
//! * Everything that renders inside a transaction needs a [`Gate`]: `settle`
//!   builds its scope from the execution context's, and the only entries to
//!   it -- `claim::apply_outcome_with` and `claim::start_execution_in_with`
//!   -- take an [`InTxn`] (an [`Txn`] or a [`Savepoint`] of one), not
//!   a connection. A `Gate` is made only here, by [`Txn::begin`] (and,
//!   for the test-only caller-owned wrappers, by a constructor that exists
//!   only with `test-util`).
//! * A render OUTSIDE a transaction (`storage::scope::scope_for`) takes a
//!   pool connection, which a transaction is not, so the I/O scope cannot be
//!   built on one.
//!
//! # Why a macro
//!
//! The body borrows the owner's locals and the attempt, and the future it
//! makes must be `Send` (the claim loop and the routes are spawned tasks). An
//! `AsyncFnMut` parameter cannot say that today (its future's `Send` is
//! checked for every lifetime and fails, rust-lang/rust#110338), so the loop
//! is written once here as a macro and the body is an inline `async` block.
//! The decisions -- how an attempt begins, and the gate -- are the functions
//! below, not macro text.

use egret_core::{Error, Result};
use sqlx::{Connection, PgConnection, PgPool, Postgres, Transaction};

use crate::storage::prefetch::Prefetch;

/// Keeps the `sqlx::Error` as the source, as `claim`'s does.
fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::Error::new(e))
}

/// Proof that whoever renders through this cache is inside an attempt whose
/// commit is [`finish`]'s: the cache of one [`transaction!`] call. Its field
/// is private, so only this module makes one. See the module doc.
///
/// **`Clone`, because every execution one attempt settles carries it**
/// (re-review n3): `claim::ExecutionCtx::gate` is owned, and a nested settle
/// builds a new context -- a loop iteration, a `Subflow` child, a replay, a
/// parent reloaded by `claim::Stack::Nested` -- that must read through the
/// SAME cache, so the owner's [`finish`] sees its misses. A clone shares the
/// `Prefetch` (an `Arc`); it is not a second cache and grants nothing a
/// `Txn` did not. Borrowing instead would put a lifetime on `ExecutionCtx`,
/// `Caller` and `Stack` and every function that holds one. The misuse a
/// clone could allow -- carrying it to a connection no `Txn` owns -- has no
/// entry: `settle` is reached only through `apply_outcome_with` and
/// `start_execution_in_with`, which take an [`InTxn`].
#[derive(Clone, Debug)]
pub(crate) struct Gate(Prefetch);

impl Gate {
    /// The cache every render under this gate reads through.
    pub(crate) fn prefetch(&self) -> &Prefetch {
        &self.0
    }

    /// For the caller-owned wrappers `apply_outcome` and `start_execution_in`,
    /// which exist only for tests: the CALLER is the gate there (each
    /// answers `Err` when `pf` has a miss pending, and its caller must not
    /// commit).
    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn checked_by_caller(pf: &Prefetch) -> Gate {
        Gate(pf.clone())
    }
}

/// One attempt of a [`transaction!`]: the transaction, which only [`finish`]
/// commits, and the call's [`Gate`].
pub(crate) struct Txn {
    tx: Transaction<'static, Postgres>,
    gate: Gate,
}

/// What renders inside an attempt: its connection and its gate. An
/// [`Txn`], or a [`Savepoint`] of one -- never a bare connection.
pub(crate) trait InTxn {
    /// The attempt's connection and gate. **Never issue `COMMIT`,
    /// `ROLLBACK` or `END` on the connection** -- only [`finish`] may end the
    /// attempt, after its miss check; a raw `COMMIT` here would commit a
    /// render that missed. Statements only.
    fn parts(&mut self) -> (&mut PgConnection, &Gate);
}

impl Txn {
    /// Begin one attempt reading through `cache`.
    pub(crate) async fn begin(
        pool: &PgPool,
        isolation: Isolation,
        cache: &Prefetch,
    ) -> Result<Txn> {
        let tx = match isolation {
            Isolation::Default => pool.begin().await,
            Isolation::RepeatableRead => {
                pool.begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ")
                    .await
            }
        }
        .map_err(db_err)?;
        Ok(Txn {
            tx,
            gate: Gate(cache.clone()),
        })
    }

    /// The attempt's connection, **for statements only** (the scheduler's
    /// fire). **Never issue `COMMIT`, `ROLLBACK` or `END` on it**: only
    /// [`finish`] ends an attempt, after its miss check, and a raw `COMMIT`
    /// here would commit a render that missed (re-review n3). Crate-private
    /// for that reason.
    pub(crate) fn conn(&mut self) -> &mut PgConnection {
        &mut self.tx
    }

    pub(crate) fn gate(&self) -> &Gate {
        &self.gate
    }

    /// A savepoint inside this attempt: what a part of the body that must
    /// fail alone (a scheduler fire, a Flow-trigger listener) renders in.
    /// Releasing it is not a commit; the attempt's is still [`finish`]'s.
    pub(crate) async fn savepoint(&mut self) -> Result<Savepoint<'_>> {
        let Txn { tx, gate } = self;
        let sp = Connection::begin(&mut **tx).await.map_err(db_err)?;
        Ok(Savepoint { sp, gate })
    }
}

impl InTxn for Txn {
    fn parts(&mut self) -> (&mut PgConnection, &Gate) {
        (&mut self.tx, &self.gate)
    }
}

/// A savepoint of an [`Txn`]: see [`Txn::savepoint`].
pub(crate) struct Savepoint<'a> {
    sp: Transaction<'a, Postgres>,
    gate: &'a Gate,
}

impl Savepoint<'_> {
    /// The savepoint's connection, for statements only: see [`Txn::conn`].
    pub(crate) fn conn(&mut self) -> &mut PgConnection {
        &mut self.sp
    }

    /// `RELEASE SAVEPOINT`: what the savepoint did joins the attempt.
    pub(crate) async fn release(self) -> Result<()> {
        self.sp.commit().await.map_err(db_err)
    }

    /// `ROLLBACK TO SAVEPOINT`.
    pub(crate) async fn rollback(self) -> Result<()> {
        self.sp.rollback().await.map_err(db_err)
    }
}

impl InTxn for Savepoint<'_> {
    fn parts(&mut self) -> (&mut PgConnection, &Gate) {
        (&mut self.sp, self.gate)
    }
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

/// **THE gate.** An attempt that recorded a miss is rolled back, whatever it
/// answered -- an `Err` included, which a placeholder may have caused -- and
/// never committed; any other is committed or rolled back as its body said,
/// and an `Err` rolls back.
pub(crate) async fn finish<T>(attempt: Txn, answered: Result<Finish<T>>) -> Result<Next<T>> {
    let Txn { tx, gate } = attempt;
    if gate.prefetch().has_pending() {
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
/// * `transaction!(pool, isolation, |at| { body }, |pf| { fetch })`: `body`
///   is the attempt, evaluating to `Result<Finish<T>>` (`?` inside it ends
///   the ATTEMPT, never the loop, so it cannot skip the gate); `fetch` runs
///   after the rollback and evaluates to `Result<Fetched>`. The whole
///   evaluates to `Result<Ended<T>>`.
/// * `transaction!(pool, |at| { body })`: `body` evaluates to `Result<T>` and
///   commits on `Ok`; the fetch is the plain `pf.fetch_pending()`. The whole
///   evaluates to `Result<T>`.
///
/// `at` is `&mut Txn`, `pf` is `&Prefetch`; a fresh `Prefetch` per use,
/// so the cache lives for this call only.
macro_rules! transaction {
    ($pool:expr, $isolation:expr, |$at:ident| $body:block, |$fpf:ident| $fetch:block) => {{
        let ended: egret_core::Result<$crate::worker::txn::Ended<_>> = async {
            let cache = $crate::storage::prefetch::Prefetch::new();
            loop {
                let mut attempt =
                    $crate::worker::txn::Txn::begin($pool, $isolation, &cache).await?;
                let answered = {
                    let $at: &mut $crate::worker::txn::Txn = &mut attempt;
                    async { $body }.await
                };
                match $crate::worker::txn::finish(attempt, answered).await? {
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
    ($pool:expr, |$at:ident| $body:block) => {{
        let ended = $crate::worker::txn::transaction!(
            $pool,
            $crate::worker::txn::Isolation::Default,
            |$at| {
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
