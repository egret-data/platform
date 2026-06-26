//! Postgres pool plumbing that isolates lakekeeper's catalog backend into its
//! own `lakekeeper` schema.
//!
//! lakekeeper-storage-postgres ships ~63 migrations run via `sqlx::migrate!`
//! and tracks them in a `_sqlx_migrations` table created with an UNQUALIFIED
//! name — it lands in whichever schema is first on the connection's
//! `search_path`. egret-metastore does the same against `public`. Running both
//! against one database+schema collides on table names and on the migration
//! tracker.
//!
//! The fix: give lakekeeper its own `lakekeeper` schema and a pool whose every
//! connection runs with `search_path = lakekeeper, public`. lakekeeper's
//! unqualified DDL (and its `_sqlx_migrations`) then lands in `lakekeeper.*`,
//! leaving egret's tables untouched in `public.*`. `public` stays second so
//! shared extensions/types (e.g. `gen_random_uuid`, `uuid-ossp`) still resolve.
//!
//! lakekeeper itself documents and regression-tests this exact mechanism (a
//! role-level `SET search_path = "<schema>", public`); we apply it per-pool
//! instead of per-role so it works without a dedicated DB role.

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{AssertSqlSafe, ConnectOptions, Connection, Executor, PgPool};
use std::str::FromStr;

/// The dedicated schema lakekeeper's catalog tables and migration tracker live in.
pub const LAKEKEEPER_SCHEMA: &str = "lakekeeper";

/// Ensure the `lakekeeper` schema exists, then build a [`PgPool`] whose every
/// connection runs with `search_path = lakekeeper, public`.
///
/// The schema is created with a one-shot connection (which itself needs no
/// special search_path) before the isolated pool is returned. Every connection
/// the pool hands out — now and after reconnects — sets the search_path via an
/// `after_connect` hook, so unqualified DDL/DML targets `lakekeeper.*`.
///
/// # Errors
/// Returns an error if the DSN is invalid, the schema cannot be created, or the
/// pool cannot connect.
pub async fn build_isolated_pool(dsn: &str) -> anyhow::Result<PgPool> {
    let opts = PgConnectOptions::from_str(dsn)?;

    // 1. Ensure the schema exists (one-shot connection on the default search_path).
    {
        let mut conn = opts.clone().connect().await?;
        // `LAKEKEEPER_SCHEMA` is a compile-time const identifier, not user input.
        conn.execute(AssertSqlSafe(format!(
            "CREATE SCHEMA IF NOT EXISTS \"{LAKEKEEPER_SCHEMA}\""
        )))
        .await?;
        conn.close().await?;
    }

    // 2. Build a pool that pins search_path on every (re)connection. We keep
    //    `public` second so extension-provided functions/types installed there
    //    still resolve from inside the lakekeeper schema.
    let set_search_path = format!("SET search_path = \"{LAKEKEEPER_SCHEMA}\", public");
    let pool = PgPoolOptions::new()
        .after_connect(move |conn, _meta| {
            // `LAKEKEEPER_SCHEMA` is a compile-time const identifier, not user input.
            let stmt = AssertSqlSafe(set_search_path.clone());
            Box::pin(async move {
                conn.execute(stmt).await?;
                Ok(())
            })
        })
        .connect_with(opts)
        .await?;

    Ok(pool)
}
