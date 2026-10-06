//! Session-level advisory locks on one pooled connection (W9a's pruner, W9b's
//! hard purge and move).
//!
//! A **session** lock outlives a transaction's commit, which a purge needs:
//! it deletes rows, commits, then deletes objects, and a writer must not take
//! the purged revision number again (and upload to its key) between the
//! commit and the object deletes (W9b spec §4.2, §8.4; plan item 7). Session
//! and transaction advisory locks on one key conflict with each other, so a
//! writer's `pg_advisory_xact_lock` waits for the session lock
//! (`tests/nsfiles_store.rs` measures it).

use egret_core::{Error, Result};
use sqlx::pool::PoolConnection;
use sqlx::{PgConnection, PgPool, Postgres};

/// A pooled connection holding session-level advisory locks. Dropped without
/// [`LockedSession::unlock`] answering `Ok`, it is detached from the pool and
/// closed (closing a session releases its locks) -- the pruner's rule: on a
/// failed unlock, a panic or a dropped future, the connection never goes back
/// to the pool still holding a lock.
pub(crate) struct LockedSession {
    conn: Option<PoolConnection<Postgres>>,
    /// Every lock was released: the connection may go back to the pool.
    unlocked: bool,
    /// The keys [`LockedSession::acquire`] took, in the order taken.
    keys: Vec<i64>,
}

impl LockedSession {
    /// A connection that holds no lock yet; the pruner takes its own
    /// `pg_try_advisory_lock` on it and sets `unlocked` itself.
    pub(super) fn hold(conn: PoolConnection<Postgres>) -> Self {
        Self {
            conn: Some(conn),
            unlocked: false,
            keys: Vec::new(),
        }
    }

    /// `pg_advisory_lock` on every key, in the order given (callers sort),
    /// one statement per key (a set-returning form over `unnest` does not
    /// guarantee the order). An error part-way drops the session, which
    /// closes the connection and so releases what was taken.
    ///
    /// `wait` bounds each key's wait (`lock_timeout`, W9b final review I-1):
    /// past it the statement fails with SQLSTATE 55P03 ([`is_lock_timeout`]),
    /// and the session -- whose `lock_timeout` was set for the session -- is
    /// dropped and so closed, never returned to the pool with the setting.
    /// Once every key is held the setting is `RESET`, so the work done under
    /// the locks is not bounded by it. `None` waits as long as it takes.
    pub(crate) async fn acquire(
        pool: &PgPool,
        keys: &[i64],
        wait: Option<std::time::Duration>,
    ) -> Result<Self> {
        let conn = pool.acquire().await.map_err(db_err)?;
        let mut session = Self::hold(conn);
        if let Some(wait) = wait {
            sqlx::query("SELECT set_config('lock_timeout', $1, false)")
                .bind(lock_timeout_value(wait))
                .execute(session.conn())
                .await
                .map_err(db_err)?;
        }
        for &key in keys {
            sqlx::query("SELECT pg_advisory_lock($1)")
                .bind(key)
                .execute(session.conn())
                .await
                .map_err(db_err)?;
            session.keys.push(key);
        }
        if wait.is_some() {
            sqlx::query("RESET lock_timeout")
                .execute(session.conn())
                .await
                .map_err(db_err)?;
        }
        Ok(session)
    }

    /// The pruner's own bookkeeping: its `pg_try_advisory_lock` answered
    /// `false` (nothing to release), or its `pg_advisory_unlock` answered
    /// `true`. The connection may go back to the pool.
    pub(super) fn mark_released(&mut self) {
        self.unlocked = true;
    }

    pub(crate) fn conn(&mut self) -> &mut PgConnection {
        self.conn.as_mut().expect("held until drop")
    }

    /// `pg_advisory_unlock` on every key, the last taken first; hands the
    /// connection back to the pool only if every unlock answered `true`.
    /// Otherwise the connection is closed (releasing whatever it holds) and
    /// the error says which key.
    pub(crate) async fn unlock(mut self) -> Result<()> {
        let keys = std::mem::take(&mut self.keys);
        for &key in keys.iter().rev() {
            let released: bool = sqlx::query_scalar("SELECT pg_advisory_unlock($1)")
                .bind(key)
                .fetch_one(self.conn())
                .await
                .map_err(db_err)?;
            if !released {
                return Err(Error::Internal(anyhow::anyhow!(
                    "advisory lock {key} was not held by this session; its connection is \
                     closed instead of returned to the pool"
                )));
            }
        }
        self.unlocked = true;
        Ok(())
    }
}

impl Drop for LockedSession {
    fn drop(&mut self) {
        if !self.unlocked {
            if let Some(conn) = self.conn.take() {
                drop(conn.detach());
            }
        }
    }
}

/// `lock_timeout`'s value for `wait`, in milliseconds; never `0`, which
/// would disable the bound.
pub(crate) fn lock_timeout_value(wait: std::time::Duration) -> String {
    format!("{}ms", wait.as_millis().max(1))
}

/// Did a statement fail because its `lock_timeout` ran out (SQLSTATE 55P03,
/// `lock_not_available`)?
pub(crate) fn is_lock_timeout(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .and_then(|d| d.code())
        .is_some_and(|c| c == "55P03")
}

/// [`is_lock_timeout`] through [`Error::Internal`], which is how
/// [`LockedSession::acquire`] answers.
pub(crate) fn is_lock_timeout_error(e: &Error) -> bool {
    match e {
        Error::Internal(a) => a.downcast_ref::<sqlx::Error>().is_some_and(is_lock_timeout),
        _ => false,
    }
}

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::Error::new(e))
}
