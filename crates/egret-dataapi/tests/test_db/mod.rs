//! One private, already-migrated database per test process.
//!
//! Every test binary in this crate used to open the single `DATABASE_URL` database and
//! never clean it. Measured 2026-08-25, after a few hours of ordinary use it held **6,771
//! roles** — and `CedarPdp::build` compiles **one Cedar template per role**, so every
//! fixture paid for all of them. The same 27-test binary took **82.25 s** against that
//! database and **7.87 s** against a fresh one, and the gap widened every time anyone ran
//! the suite. `pdp_hot_reload` failed outright past roughly ten thousand rows.
//!
//! Isolation is affordable because the template is migrated once: a clone costs **~150 ms**
//! against **97 s** to run the 129 migrations from scratch. It also makes running test
//! binaries in parallel safe, which sharing one database never was.

use egret_metastore::Db;
use uuid::Uuid;

/// The database this test process owns, cloned from a shared migrated template.
///
/// # Why a private database per process rather than one shared `egret_test`
///
/// Every test in this workspace used to write into one database that nothing ever
/// cleaned. Measured 2026-08-25 after a few hours of use it held **6,771 roles**, and
/// `CedarPdp::build` compiles **one Cedar template per role** — so every `fixture()` call
/// paid for all of them. The same 27-test binary took **82.25s** against that database
/// and **7.87s** against a fresh one. The suite was getting slower every time anyone ran
/// it, forever, and `pdp_hot_reload` failed outright past roughly ten thousand rows.
///
/// A clone is cheap because the template is already migrated: **~150 ms**, against 97 s to
/// run the 129 migrations from scratch.
///
/// # How a new migration reaches the template
///
/// The template is migrated once per process, before the clone. `db.migrate()` is
/// idempotent, so a migration added since the template was built is applied to it and
/// every later clone carries it. An **edited** migration is different: sqlx compares
/// checksums and refuses, and the recovery is to drop the template so the next run
/// rebuilds it — that refusal is deliberate, because silently re-running an edited
/// migration is how two developers end up with different schemas under the same name.
static TEST_DB: tokio::sync::OnceCell<Option<String>> = tokio::sync::OnceCell::const_new();

const TEMPLATE_DB: &str = "egret_test_template";

/// Everything below reconnects to `postgres` rather than reusing the caller's pool:
/// `CREATE DATABASE ... TEMPLATE x` is refused while anything is connected to `x`.
async fn admin_conn(url: &str) -> Option<sqlx::PgConnection> {
    use sqlx::Connection;
    let admin_url = url
        .rsplit_once('/')
        .map(|(base, _)| format!("{base}/postgres"))?;
    sqlx::PgConnection::connect(&admin_url).await.ok()
}

async fn ensure_template(url: &str) -> Option<()> {
    let mut admin = admin_conn(url).await?;
    // Two test binaries may start at the same instant. The lock makes the
    // create-then-migrate pair atomic across processes; without it the second one
    // meets a half-migrated template and fails in a way that reads like a schema bug.
    sqlx::query("SELECT pg_advisory_lock(8_25_2026)")
        .execute(&mut admin)
        .await
        .ok()?;

    let exists: (bool,) =
        sqlx::query_as("SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(TEMPLATE_DB)
            .fetch_one(&mut admin)
            .await
            .ok()?;
    if !exists.0 {
        // `AssertSqlSafe` because the only interpolation is `TEMPLATE_DB`, a `const`
        // in this file. `CREATE DATABASE` cannot take a bind parameter for its name.
        let stmt = format!(r#"CREATE DATABASE "{TEMPLATE_DB}""#);
        sqlx::query(sqlx::AssertSqlSafe(stmt))
            .execute(&mut admin)
            .await
            .ok()?;
    }

    // Migrate the template itself, then drop the connection so it can be cloned.
    let tmpl_url = url
        .rsplit_once('/')
        .map(|(b, _)| format!("{b}/{TEMPLATE_DB}"))?;
    let tmpl = Db::connect(&tmpl_url, 1).await.expect("connect template");
    tmpl.migrate().await.expect(
        "migrate the template — if this fails on a checksum, an applied migration was \
         edited; drop the egret_test_template database and re-run",
    );
    drop(tmpl);

    sqlx::query("SELECT pg_advisory_unlock(8_25_2026)")
        .execute(&mut admin)
        .await
        .ok()?;
    Some(())
}

/// Drop clones left behind by processes that were killed before their own cleanup ran.
/// Without this the accumulation problem simply moves from rows to databases.
async fn drop_abandoned_clones(url: &str) {
    let Some(mut admin) = admin_conn(url).await else {
        return;
    };
    // A REGEX, not `LIKE`. `_` is a single-character wildcard in `LIKE`, so the obvious
    // pattern `egret_t_%` also matches `egret_test` — and this function dropped the
    // shared database it was written to protect. The anchored pattern below can only
    // match a name this module generates: the prefix, then exactly 32 hex characters
    // from `Uuid::simple()`.
    let stale: Vec<(String,)> = sqlx::query_as(
        "SELECT datname FROM pg_database d \
         WHERE datname ~ '^egret_t_[0-9a-f]{32}$' \
           AND NOT EXISTS (SELECT 1 FROM pg_stat_activity a WHERE a.datid = d.oid)",
    )
    .fetch_all(&mut admin)
    .await
    .unwrap_or_default();
    for (name,) in stale {
        // `name` came from `pg_database` one query ago and matched `egret_t_%`, so it
        // is a name Postgres itself produced — not caller input.
        let stmt = format!(r#"DROP DATABASE IF EXISTS "{name}""#);
        let _ = sqlx::query(sqlx::AssertSqlSafe(stmt))
            .execute(&mut admin)
            .await;
    }
}

async fn process_db_url() -> Option<String> {
    let url = std::env::var("DATABASE_URL").ok()?;
    ensure_template(&url).await?;
    drop_abandoned_clones(&url).await;

    let name = format!("egret_t_{}", Uuid::new_v4().simple());
    let mut admin = admin_conn(&url).await?;
    // `name` is `egret_t_` plus a `Uuid::simple()`, which is hex only; `TEMPLATE_DB`
    // is a `const`. Neither can carry an identifier break, and `CREATE DATABASE` has no
    // bind form for either position.
    let stmt = format!(r#"CREATE DATABASE "{name}" TEMPLATE "{TEMPLATE_DB}""#);
    sqlx::query(sqlx::AssertSqlSafe(stmt))
        .execute(&mut admin)
        .await
        .ok()?;
    url.rsplit_once('/')
        .map(|(base, _)| format!("{base}/{name}"))
}

/// The URL this process should use. `None` when `DATABASE_URL` is unset, which is how the
/// DB-gated tests decide to skip.
///
/// # Why the answer is cached in an environment variable and not only in the static
///
/// This module is included by `#[path]` into several test binaries, and one of them also
/// pulls in `serving_common`, which includes it too. Two copies of a module are two
/// distinct `static`s — so a per-copy `OnceCell` alone would hand the same process two
/// different databases, and a fixture built through one path would not see rows written
/// through the other. An environment variable is process-global no matter how many copies
/// exist, so every copy converges on the first answer.
pub async fn url() -> Option<String> {
    const CACHE: &str = "EGRET_TEST_DB_URL";
    if let Ok(existing) = std::env::var(CACHE) {
        return Some(existing);
    }
    let computed = TEST_DB
        .get_or_init(|| async { process_db_url().await })
        .await
        .clone()?;
    // SAFETY-ish: tests are single-process; this runs before any thread reads it, and a
    // second writer would be writing the identical value it just read back.
    unsafe { std::env::set_var(CACHE, &computed) };
    Some(computed)
}
