//! One private, already-migrated database per test process, for every crate.
//!
//! Test binaries in this workspace open the single `DATABASE_URL` database and never
//! clean it. Measured 2026-08-25 in `egret-dataapi`, after a few hours of ordinary use it
//! held **6,771 roles** — and `CedarPdp::build` compiles **one Cedar template per role**,
//! so every fixture paid for all of them. The same 27-test binary took **82.25 s** against
//! that database and **7.87 s** against a fresh one, and the gap widened every time anyone
//! ran the suite. `pdp_hot_reload` failed outright past roughly ten thousand rows.
//!
//! Isolation is affordable because the template is migrated once and then cloned: a clone
//! costs **~150 ms**. It also makes running test binaries in parallel safe, which sharing
//! one database never was.
//!
//! # Using it
//!
//! Add it as a dev-dependency and replace the `DATABASE_URL` read in the crate's `connect`
//! helper:
//!
//! ```ignore
//! // before
//! let url = std::env::var("DATABASE_URL").ok()?;
//! // after
//! let url = egret_test_db::url().await?;
//! ```
//!
//! # A skip is not a pass
//!
//! `url()` used to return `None` when `DATABASE_URL` was unset, and every caller turned
//! that into an early `return` — so **a DB-gated test that skipped reported `ok` while
//! asserting nothing**, and the only way to tell it apart from a real pass was the
//! duration. Measured on `cargo test -p egret-dataapi`, same commit, same machine: with a
//! database, `373 passed` in **282.69 s**; without one, the identical `373 passed` in
//! **11.31 s**, of which 315 tests executed no assertion at all.
//!
//! So `url()` now **panics** instead, and skipping requires [`ALLOW_SKIP_VAR`]
//! (`EGRET_ALLOW_DB_SKIP=1`) — which puts the skip in the invocation rather than leaving
//! it to be inferred from the wall clock. That claim only holds if the variable cannot
//! arrive from somewhere nobody looks, so an opt-out set by a `.env` file is **refused**
//! rather than honoured; see [`ALLOW_SKIP_VAR`]. See `docs/en/testing/db-gated-tests.md`.
//!
//! # Why it is a crate and not a `#[path]` module
//!
//! It began as `crates/egret-dataapi/tests/test_db/mod.rs`, textually included into each of
//! that crate's test binaries. A `tests/` directory is reachable only from its own crate,
//! so seventeen other crates could not use it; and textual inclusion meant several *copies*
//! of the module inside one binary, each with its own `static`s. As a crate it is compiled
//! once and linked once, which is what removes that whole class of problem — see
//! [`process_db_name`] for the part of the old workaround that is still worth keeping.

use egret_metastore::Db;

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
/// A clone is cheap because the template is already migrated. Re-measured 2026-08-26 on
/// the extraction: applying the 129 migrations to an empty database takes **~7 s**
/// in-process, and a `CREATE DATABASE ... TEMPLATE` clone of the result takes
/// **~200 ms** — so the whole-suite cost of isolation is one migration run plus 150 ms
/// per test binary. (An earlier revision of this doc said 97 s for the migrations; that
/// figure did not survive re-measurement and has been corrected rather than repeated.)
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
fn admin_url_of(url: &str) -> Option<String> {
    url.rsplit_once('/')
        .map(|(base, _)| format!("{base}/postgres"))
}

async fn admin_conn(url: &str) -> Option<sqlx::PgConnection> {
    use sqlx::Connection;
    sqlx::PgConnection::connect(&admin_url_of(url)?).await.ok()
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

/// The pattern that decides what this module is allowed to drop, in one place.
///
/// A REGEX, not `LIKE`. `_` is a single-character wildcard in `LIKE`, so the obvious
/// pattern `egret_t_%` also matches `egret_test` — and an earlier cleanup helper in this
/// repo dropped the shared database it was written to protect, then reported a 2.01 s
/// suite as a triumph *because* the database was gone. The anchored pattern below can
/// only match a name this module generates: the prefix, then exactly 32 hex characters.
const CLONE_PATTERN: &str = "^egret_t_[0-9a-f]{32}$";

/// Drop clones left behind by processes that were killed before their own cleanup ran.
/// Without this the accumulation problem simply moves from rows to databases.
///
/// # It runs under the lock, and it takes the caller's connection to say so
///
/// "No row in `pg_stat_activity`" is only evidence of abandonment while nothing can be
/// BETWEEN creating a clone and connecting to it. Process A used to create its database
/// and connect a moment later; process B starting inside that window saw A's brand-new
/// database with no sessions on it, dropped it, and A then failed on a missing relation —
/// which reads exactly like a schema bug and sends the reader to the migrations.
///
/// So this takes the admin connection that holds `pg_advisory_lock(8_25_2026)` rather
/// than opening its own: there is no way to call it outside the lock, and the window it
/// used to race with is inside the same critical section (see [`process_db_url`]).
///
/// # This is the backstop, not the cleanup
///
/// It only runs when a *later* process starts, so on its own it leaves the last run's
/// clones on disk until the next run — measured, one 12 MB database sitting idle for days.
/// [`register_exit_drop`] is what makes the common case leave nothing behind; this stays
/// for the case an exit hook cannot cover, a process killed with `SIGKILL` or aborted.
async fn drop_abandoned_clones(admin: &mut sqlx::PgConnection) {
    let stale: Vec<(String,)> = sqlx::query_as(
        "SELECT datname FROM pg_database d \
         WHERE datname ~ $1 \
           AND NOT EXISTS (SELECT 1 FROM pg_stat_activity a WHERE a.datid = d.oid)",
    )
    .bind(CLONE_PATTERN)
    .fetch_all(&mut *admin)
    .await
    .unwrap_or_default();
    for (name,) in stale {
        // `name` came from `pg_database` one query ago and matched `CLONE_PATTERN`, so
        // it is a name Postgres itself produced — not caller input.
        let stmt = format!(r#"DROP DATABASE IF EXISTS "{name}""#);
        let _ = sqlx::query(sqlx::AssertSqlSafe(stmt))
            .execute(&mut *admin)
            .await;
    }
}

/// This process's database name: `egret_t_` and the process id as 32 hex digits.
///
/// **Derived rather than random**, which the crate extraction did not make pointless.
/// It buys two things. The create below can then be written to tolerate the database
/// already existing, so nothing has to distinguish "I made it" from "it was there"; and
/// [`register_exit_drop`] can name the database to drop without reading any shared state
/// that the exiting process may already have torn down.
///
/// It also removed an `unsafe { std::env::set_var }`, which was how the pre-crate version
/// shared the answer between its several textual copies — under a multi-threaded tokio
/// runtime, beside a `sqlx` and a `dotenvy` that read the environment on other threads,
/// which is precisely the data race that `unsafe` marks.
///
/// A live process cannot collide with another live one, because a pid identifies exactly
/// one running process. A DEAD process's database with the same pid is swept first — it
/// has no sessions — and the sweep runs under the lock immediately before the create.
///
/// 32 hex digits because [`CLONE_PATTERN`] requires exactly that many, which is what keeps
/// the sweep unable to match `egret_test` itself.
fn process_db_name() -> String {
    format!("egret_t_{:032x}", std::process::id())
}

/// A connection to the clone, held open for the life of the process.
///
/// It is not for querying — every test opens its own pool. It exists so this process's
/// database always has a row in `pg_stat_activity`, which is what
/// [`drop_abandoned_clones`] reads to tell a live clone from an abandoned one. Between
/// test cases a pool can legitimately hold zero connections, and without this a sweep in
/// another process would drop a database that is very much in use.
static SENTINEL: tokio::sync::OnceCell<Option<sqlx::PgConnection>> =
    tokio::sync::OnceCell::const_new();

/// Drop this process's clone as the process exits.
///
/// # Why an exit hook at all — the sweep was never cleanup
///
/// [`drop_abandoned_clones`] runs at the *start* of a test process, so the last clone of
/// any run survives until somebody runs the suite again. Measured 2026-08-26 on a machine
/// with no suite running: `SELECT count(*) FROM pg_database WHERE datname ~
/// '^egret_t_[0-9a-f]{32}$'` returned 1, a 12 MB database whose owning pid had been dead
/// for days. The sweep query itself was correct — running it by hand selected exactly that
/// database — it simply had no occasion to run. With test binaries running in parallel the
/// residue is one per process alive at the end of the run, not one overall.
///
/// # Why `atexit`, and why it hands the work to a thread
///
/// Nothing in a test binary owns this database in a scope that ends: `main` belongs to
/// libtest, and Rust does not run destructors for `static`s. `libc::atexit` is the hook
/// both of a test binary's exit paths go through — returning from `main`, and the
/// `std::process::exit` libtest calls when a test fails.
///
/// The handler itself can do almost nothing, though. By the time `atexit` callbacks run,
/// the exiting thread's thread-local storage has already been torn down, and the first
/// version of this — which built a `current_thread` runtime right there — printed
///
/// ```text
/// thread 'main' panicked: use of std::thread::current() is not possible after the
/// thread's local data has been destroyed
/// ```
///
/// into the test output and dropped nothing. So the runtime lives on a worker thread
/// spawned while the process is healthy, whose TLS is still intact; the handler only takes
/// a mutex, signals a condvar, and waits. Mutex and condvar touch no thread-local state.
///
/// It is deliberately incapable of failing the test run: the handler gives up after ten
/// seconds and every error is ignored. A cleanup that can turn a green suite red is worse
/// than a leaked database.
fn register_exit_drop(admin_url: String, name: String) {
    use std::sync::{Condvar, Mutex};

    // `'static` because the `atexit` callback takes no argument and outlives every frame.
    struct Reaper {
        /// `false` until the exit handler asks for the drop, then `true` again when the
        /// worker has finished — the handler waits for the second transition.
        state: Mutex<(bool, bool)>,
        cv: Condvar,
    }
    static REAPER: std::sync::OnceLock<&'static Reaper> = std::sync::OnceLock::new();

    let reaper: &'static Reaper = Box::leak(Box::new(Reaper {
        state: Mutex::new((false, false)),
        cv: Condvar::new(),
    }));
    if REAPER.set(reaper).is_err() {
        return;
    }

    let spawned = std::thread::Builder::new()
        .name("egret-test-db-reaper".into())
        .spawn(move || {
            let mut guard = reaper.state.lock().unwrap_or_else(|e| e.into_inner());
            while !guard.0 {
                guard = reaper.cv.wait(guard).unwrap_or_else(|e| e.into_inner());
            }
            drop(guard);

            // A fresh current-thread runtime. The runtimes the tests built are gone, and
            // so is the one the `SENTINEL` connection was made on — its socket is still
            // open, which is why the drop below has to be `WITH (FORCE)`.
            if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                rt.block_on(async {
                    let work = async {
                        use sqlx::Connection;
                        let mut admin = sqlx::PgConnection::connect(&admin_url).await.ok()?;
                        // `name` is `egret_t_` plus hex from `process_db_name`; it cannot
                        // carry an identifier break, and `DROP DATABASE` has no bind form
                        // for a name.
                        let stmt = format!(r#"DROP DATABASE IF EXISTS "{name}" WITH (FORCE)"#);
                        sqlx::query(sqlx::AssertSqlSafe(stmt))
                            .execute(&mut admin)
                            .await
                            .ok()
                    };
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(8), work).await;
                });
            }

            let mut guard = reaper.state.lock().unwrap_or_else(|e| e.into_inner());
            guard.1 = true;
            reaper.cv.notify_all();
        })
        .is_ok();

    if !spawned {
        return;
    }

    extern "C" fn drop_clone_at_exit() {
        // An escape hatch for the developer who wants to open the database and look at it.
        if std::env::var_os("EGRET_TEST_DB_KEEP").is_some() {
            return;
        }
        let Some(reaper) = REAPER.get() else {
            return;
        };
        let Ok(mut guard) = reaper.state.lock() else {
            return;
        };
        guard.0 = true;
        reaper.cv.notify_all();
        while !guard.1 {
            let Ok((g, timeout)) = reaper
                .cv
                .wait_timeout(guard, std::time::Duration::from_secs(10))
            else {
                return;
            };
            guard = g;
            if timeout.timed_out() {
                return;
            }
        }
    }

    // SAFETY: registering a callback. The callback below only takes a mutex and waits on
    // a condvar, neither of which reads the thread-local state `atexit` has already torn
    // down; the actual work happens on the thread spawned above.
    unsafe {
        libc::atexit(drop_clone_at_exit);
    }
}

async fn process_db_url() -> Option<String> {
    let url = std::env::var("DATABASE_URL").ok()?;
    ensure_template(&url).await?;

    let mut admin = admin_conn(&url).await?;
    // ONE critical section over sweep-create-connect. Every step below is a step another
    // process's sweep must not be able to interleave with: it decides what is abandoned
    // from `pg_stat_activity`, and this process has no session on its database until the
    // sentinel connects. `ensure_template` takes and releases the same lock above; the
    // two are sequential, and advisory locks are re-entrant per session anyway.
    sqlx::query("SELECT pg_advisory_lock(8_25_2026)")
        .execute(&mut admin)
        .await
        .ok()?;
    drop_abandoned_clones(&mut admin).await;

    let name = process_db_name();
    // `name` is `egret_t_` plus hex; `TEMPLATE_DB` is a `const`. Neither can carry an
    // identifier break, and `CREATE DATABASE` has no bind form for either position.
    //
    // A failure here is not fatal on its own: the name is derived, so a caller that
    // reaches this a second time finds the database already created. The connect below
    // is what decides whether this process has a usable database.
    let stmt = format!(r#"CREATE DATABASE "{name}" TEMPLATE "{TEMPLATE_DB}""#);
    let _ = sqlx::query(sqlx::AssertSqlSafe(stmt))
        .execute(&mut admin)
        .await;

    let cloned = url
        .rsplit_once('/')
        .map(|(base, _)| format!("{base}/{name}"))?;
    let held = {
        use sqlx::Connection;
        sqlx::PgConnection::connect(&cloned).await.ok()
    };
    let connected = held.is_some();
    SENTINEL.get_or_init(|| async { held }).await;

    // Registered only once the clone is known to exist and to be reachable, so the
    // handler can never name a database this process did not create.
    if connected {
        if let Some(admin_url) = admin_url_of(&url) {
            register_exit_drop(admin_url, name);
        }
    }

    sqlx::query("SELECT pg_advisory_unlock(8_25_2026)")
        .execute(&mut admin)
        .await
        .ok();
    connected.then_some(cloned)
}

/// Fail loudly when `#[sqlx::test]` would silently fall back to the dev database.
///
/// `#[sqlx::test]` does not use anything in this crate — it creates its own scratch
/// database per test, which is a stronger isolation than a clone per process, so those
/// tests were deliberately left alone. It does, however, resolve `DATABASE_URL`
/// **through dotenvy, unconditionally**. An unexported variable therefore does not make
/// those tests skip: it makes them read `.env`, and this repo's `.env` (like its
/// `.env.example`) points `DATABASE_URL` at the **dev** database. The scratch databases
/// land on the dev server and a `_sqlx_test` bookkeeping schema is created inside the dev
/// database itself — which is how this was found: 2026-08-26, `SELECT nspname FROM
/// pg_namespace WHERE nspname LIKE '\_sqlx%'` returned a row in `egret`, not only in
/// `egret_test`.
///
/// The check is narrow on purpose. It complains only about the *silent* case — no
/// exported variable, but a `.env` at or above the working directory that supplies one.
/// With no `.env` there is nothing to fall back to and sqlx fails on its own with a clear
/// message, so this stays quiet rather than duplicating it.
///
/// # Panics
///
/// When `DATABASE_URL` is unset and a `.env` would supply one.
pub fn assert_database_url_is_exported() {
    if std::env::var_os("DATABASE_URL").is_some() {
        return;
    }
    let dotenv = dotenv_file_setting("DATABASE_URL");
    assert!(
        dotenv.is_none(),
        "DATABASE_URL is not exported, and {} sets it. `#[sqlx::test]` reads it through \
         dotenvy, so this run would create its scratch databases against the DEV database \
         instead of skipping. Export it first:\n  \
         export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test",
        dotenv
            .as_deref()
            .unwrap_or(std::path::Path::new(""))
            .display()
    );
}

/// The nearest `.env` at or above the working directory that assigns `var`, if any.
///
/// Reads the *file*, deliberately, rather than the process environment — because the two
/// mechanisms this crate has to defend against both work by putting a `.env` value into the
/// environment where nobody typed it: `just`'s `set dotenv-load := true` does it for every
/// recipe, and `dotenvy::dotenv()` (which `#[sqlx::test]` calls unconditionally) does it for
/// every variable in the file, not only `DATABASE_URL`. By the time such a value is visible
/// in `std::env` it is indistinguishable from one the operator typed. The file is the only
/// place the difference still exists.
fn dotenv_file_setting(var: &str) -> Option<std::path::PathBuf> {
    dotenv_file_setting_from(&std::env::current_dir().ok()?, var)
}

/// [`dotenv_file_setting`] with the starting directory passed in, so the ancestor walk is
/// testable against a temporary tree instead of the process' one global working directory.
fn dotenv_file_setting_from(start: &std::path::Path, var: &str) -> Option<std::path::PathBuf> {
    let prefix = format!("{var}=");
    start.ancestors().map(|d| d.join(".env")).find(|p| {
        std::fs::read_to_string(p)
            .is_ok_and(|t| t.lines().any(|l| l.trim_start().starts_with(&prefix)))
    })
}

/// The variable that turns "I have no database" from a silent pass into a deliberate act.
///
/// Set it to anything (`EGRET_ALLOW_DB_SKIP=1`) and [`url`] goes back to returning `None`,
/// so every DB-gated suite self-skips exactly as it used to. Leave it unset and a run with
/// no usable database **panics** — which is the whole point: a suite that asserted nothing
/// must not be readable as evidence that it passed.
///
/// # It may not come from a `.env` file
///
/// The whole claim this variable makes is that **the skip appears in the invocation that
/// produced the green**. A `.env` breaks that claim in one untracked line: `justfile` has
/// `set dotenv-load := true`, so `.env` enters the environment of *every* recipe, and
/// `#[sqlx::test]`'s `dotenvy::dotenv()` loads the whole file into the process environment
/// besides. Either route makes `just check` report a full green having asserted nothing,
/// and the skip's `eprintln!` is swallowed by libtest because the test *passes* — so
/// nothing at all is visible.
///
/// So an opt-out attributable to a `.env` is **refused**, by [`opt_out_verdict`], which
/// reads the file rather than the environment. That is a stronger guarantee than
/// scrubbing the variable in each recipe: it cannot be forgotten when someone adds the
/// next recipe, and it holds for a bare `cargo test` too. What it cannot see is a variable
/// exported in a shell profile, which no in-process check can tell from a typed one — the
/// `env -u` in `justfile`'s test recipes covers that half, and
/// `tests/every_test_recipe_refuses_the_skip.rs` keeps it from being dropped.
pub const ALLOW_SKIP_VAR: &str = "EGRET_ALLOW_DB_SKIP";

/// Whether the opt-out counts, given the two facts that decide it.
///
/// A pure function over `(the variable is in the environment, a .env assigns it)` for the
/// same reason [`skip_verdict`] is one: flipping the real variable in a test is `unsafe`
/// and racy beside the `sqlx`/`dotenvy` environment reads on other threads.
///
/// `Ok(true)` means "skip, the operator asked for it"; `Ok(false)` means "nobody asked";
/// `Err` carries the message the caller panics with. Refusal is deliberately conservative:
/// once the file sets it, an opt-out the operator *also* typed is indistinguishable from
/// one they did not, so both are refused and the remedy is to delete the line.
fn opt_out_verdict(in_env: bool, dotenv: Option<&std::path::Path>) -> Result<bool, String> {
    if !in_env {
        return Ok(false);
    }
    let Some(path) = dotenv else {
        return Ok(true);
    };
    Err(format!(
        "{ALLOW_SKIP_VAR} is set by {}, so this run cannot show that anyone ASKED to skip \
         the DB-gated suites.\n\
         \n\
         That file enters the environment of every `just` recipe (`set dotenv-load := \
         true`) and of every `#[sqlx::test]` binary (`dotenvy::dotenv()` loads the whole \
         file, not only DATABASE_URL) — so one untracked line there makes `just check` \
         report a full green having asserted nothing at all, and the skip message is \
         swallowed by libtest because the test PASSES.\n\
         \n\
         Delete the {ALLOW_SKIP_VAR} line from that file. Put it in the command instead, \
         where the green and the reason for it are read together:\n\
         \x20     env -u DATABASE_URL {ALLOW_SKIP_VAR}=1 cargo test --workspace",
        path.display()
    ))
}

/// # Panics
///
/// When a `.env` at or above the working directory assigns [`ALLOW_SKIP_VAR`]. This is the
/// only path in the crate that consults the variable, so there is no arrangement in which
/// the opt-out is honoured without this check having run.
fn allow_skip() -> bool {
    let in_env = std::env::var_os(ALLOW_SKIP_VAR).is_some();
    match opt_out_verdict(in_env, dotenv_file_setting(ALLOW_SKIP_VAR).as_deref()) {
        Ok(v) => v,
        Err(msg) => panic!("{msg}"),
    }
}

/// What a run with no usable database should do.
///
/// A pure function over the two facts that decide it, so the matrix is unit-testable
/// without touching the process environment — `std::env::set_var` is `unsafe` and racy
/// beside the `sqlx`/`dotenvy` reads happening on other threads (see [`process_db_name`]'s
/// note), so a test that flipped the real variable would be a worse test than no test.
///
/// `Ok(())` means "skip quietly, the operator asked for it". `Err` carries the message the
/// caller panics with.
fn skip_verdict(has_database_url: bool, allow_skip: bool) -> Result<(), String> {
    if allow_skip {
        return Ok(());
    }
    let cause = if has_database_url {
        "DATABASE_URL is set, but this process could not obtain a test database from it \
         (Postgres unreachable, or the template could not be migrated)"
    } else {
        "DATABASE_URL is not set, so there is no database to run against"
    };
    Err(format!(
        "{cause}.\n\
         \n\
         This used to make the suite SKIP — and a skipped DB-gated test reports `ok` while \
         asserting nothing, so the only way to tell a real pass from an empty one was to \
         squint at the duration. It now fails instead. Pick one:\n\
         \n\
         \x20 1. Run against a database (what CI does):\n\
         \x20      just up\n\
         \x20      export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test\n\
         \n\
         \x20 2. Deliberately skip every DB-gated suite, visibly, in the invocation:\n\
         \x20      {ALLOW_SKIP_VAR}=1 cargo test --workspace\n\
         \n\
         Never point DATABASE_URL at the dev database `egret`: `#[sqlx::test]` resolves it \
         through dotenvy and will create its scratch schemas inside it."
    ))
}

/// The URL this process should use.
///
/// # This is where a green suite stops being able to lie
///
/// It used to return `None` whenever `DATABASE_URL` was unset, and every DB-gated suite in
/// the workspace turned that `None` into an early `return` — a test that asserted nothing
/// and reported `ok`. Twenty-three integration binaries in `egret-dataapi` alone behave
/// that way, and an audit of that feature had to caveat nearly every finding with "the
/// suite reports `ok` but may have asserted nothing".
///
/// So the default is now to **panic**, and skipping requires [`ALLOW_SKIP_VAR`] — which
/// puts the skip in the command line that produced the green, rather than leaving it
/// inferable only from the wall clock. CI already provides Postgres
/// (`.github/workflows/ci.yml` runs a `postgres:16` service and sets `DATABASE_URL` at the
/// workflow's top-level `env:`, which every job inherits), so this does not change what CI
/// does; it changes what a laptop with no database does.
///
/// The same panic covers `DATABASE_URL` set but unusable. That case skipped silently too,
/// and it is strictly worse: it is the shape a CI run takes when the database service
/// failed to come up, and it went green.
///
/// Cheap to call repeatedly: the clone is created once per process and every later call
/// returns the same string.
///
/// # Panics
///
/// When no test database can be obtained and [`ALLOW_SKIP_VAR`] is unset.
pub async fn url() -> Option<String> {
    // Checked before the `OnceCell` so the unset case costs nothing and cannot be memoised
    // into a `None` that a later, better-configured call would inherit.
    if std::env::var_os("DATABASE_URL").is_none() {
        if let Err(msg) = skip_verdict(false, allow_skip()) {
            panic!("{msg}");
        }
        return None;
    }
    match TEST_DB
        .get_or_init(|| async { process_db_url().await })
        .await
        .clone()
    {
        Some(u) => Some(u),
        None => {
            if let Err(msg) = skip_verdict(true, allow_skip()) {
                panic!("{msg}");
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        dotenv_file_setting_from, opt_out_verdict, process_db_name, skip_verdict, ALLOW_SKIP_VAR,
        CLONE_PATTERN,
    };
    use std::path::{Path, PathBuf};

    /// A unique scratch directory, without a `tempfile` dependency this crate does not
    /// otherwise need. Removed by the caller; a leak here is a directory, not a database.
    fn scratch(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("egret_test_db_{tag}_{nanos}"));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    /// The bypass this whole variable exists to prevent, stated as a matrix.
    ///
    /// The mutation that fails it is making `opt_out_verdict` ignore its `dotenv`
    /// argument — i.e. honouring an opt-out that a `.env` file supplied, which is the
    /// one-line silent bypass through the exact mechanism the panic just closed.
    #[test]
    fn an_opt_out_a_dotenv_file_could_have_supplied_is_refused() {
        assert_eq!(
            opt_out_verdict(false, None),
            Ok(false),
            "unset and no file: nobody asked to skip"
        );
        assert_eq!(
            opt_out_verdict(false, Some(Path::new("/repo/.env"))),
            Ok(false),
            "a file that sets it changes nothing while the variable is not in the \
             environment — refusing here would fail runs that are not skipping at all"
        );
        assert_eq!(
            opt_out_verdict(true, None),
            Ok(true),
            "typed on the command line, with no file that could have supplied it: this is \
             the supported way to skip and it must keep working"
        );
        let refused = opt_out_verdict(true, Some(Path::new("/repo/.env")))
            .expect_err("a .env-supplied opt-out must be refused, not honoured");
        assert!(
            refused.contains("/repo/.env"),
            "must name the file to delete the line from: {refused}"
        );
        assert!(
            refused.contains(ALLOW_SKIP_VAR),
            "must name the variable: {refused}"
        );
        assert!(
            refused.contains("cargo test"),
            "must show the invocation that is still allowed, or the refusal is a wall: \
             {refused}"
        );
    }

    /// `just`'s `set dotenv-load` and `dotenvy` both search upwards, so the check has to
    /// as well: an agent working in `.claude/worktrees/<id>` is several levels below the
    /// `.env` that would supply the value.
    #[test]
    fn the_dotenv_search_walks_up_from_the_working_directory() {
        let root = scratch("dotenv_walk");
        let deep = root.join("a/b/c");
        std::fs::create_dir_all(&deep).expect("create nested dirs");

        assert_eq!(
            dotenv_file_setting_from(&deep, ALLOW_SKIP_VAR),
            None,
            "no .env anywhere above: nothing to attribute the opt-out to"
        );

        let env_file = root.join(".env");
        std::fs::write(&env_file, "DATABASE_URL=postgres://x\nUNRELATED=1\n").expect("write .env");
        assert_eq!(
            dotenv_file_setting_from(&deep, ALLOW_SKIP_VAR),
            None,
            "a .env that does not assign the variable must not be reported — otherwise \
             every developer with a .env loses the opt-out for no reason"
        );

        std::fs::write(
            &env_file,
            format!("DATABASE_URL=postgres://x\n{ALLOW_SKIP_VAR}=1\n"),
        )
        .expect("write .env");
        assert_eq!(
            dotenv_file_setting_from(&deep, ALLOW_SKIP_VAR).as_deref(),
            Some(env_file.as_path()),
            "the ancestor .env that assigns it must be found from three levels down"
        );

        // A name that merely starts with the variable is a different variable.
        std::fs::write(&env_file, format!("{ALLOW_SKIP_VAR}_NOT=1\n")).expect("write .env");
        assert_eq!(
            dotenv_file_setting_from(&deep, ALLOW_SKIP_VAR),
            None,
            "the match is on `NAME=`, not on a prefix of the name"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The matrix that decides whether a database-less run is a pass or a failure.
    ///
    /// Both `false` rows are the ones that matter and neither existed before: a run with
    /// no `DATABASE_URL`, and a run where `DATABASE_URL` is set but yields nothing, both
    /// used to return `None` and let every DB-gated suite report `ok` having asserted
    /// nothing. The mutation that fails this test is making `skip_verdict` return `Ok(())`
    /// unconditionally — i.e. restoring the old behaviour.
    #[test]
    fn only_an_explicit_opt_out_turns_a_missing_database_into_a_skip() {
        assert!(
            skip_verdict(false, true).is_ok(),
            "with the opt-out set, no database must still SKIP — otherwise there is no way \
             to run the workspace suite without Postgres"
        );
        assert!(
            skip_verdict(true, true).is_ok(),
            "the opt-out is one lever and covers the unusable-database case too"
        );
        assert!(
            skip_verdict(false, false).is_err(),
            "no DATABASE_URL and no opt-out must FAIL, not skip"
        );
        assert!(
            skip_verdict(true, false).is_err(),
            "DATABASE_URL set but unusable is the shape a CI run takes when the database \
             service failed to start — it must never be green"
        );
    }

    /// The message is the whole remedy, so it has to carry both ways out.
    ///
    /// Without this the change is a wall: a developer with no Postgres sees a failure and
    /// no path forward, which is exactly the outcome this package was told not to produce.
    #[test]
    fn the_failure_message_names_both_ways_forward() {
        let msg = skip_verdict(false, false).expect_err("must fail without the opt-out");
        assert!(
            msg.contains(ALLOW_SKIP_VAR),
            "must name the opt-out variable: {msg}"
        );
        assert!(
            msg.contains("postgres://egret:egret@localhost:5432/egret_test"),
            "must name the test database, not leave the reader to guess: {msg}"
        );
        // The database that must never be named as the target — pointing tests at it has
        // previously left the dev server unable to boot.
        assert!(
            !msg.contains("localhost:5432/egret\n") && !msg.contains("localhost:5432/egret "),
            "must not suggest the DEV database: {msg}"
        );

        let unusable = skip_verdict(true, false).expect_err("must fail without the opt-out");
        assert_ne!(
            msg, unusable,
            "the two causes must read differently — 'unset' and 'set but unreachable' send \
             the reader to different places"
        );
    }

    /// The invariant that keeps [`super::drop_abandoned_clones`] unable to reach the
    /// shared database: a clone name is the prefix plus **exactly** 32 lowercase hex
    /// digits, and `egret_test` is not of that shape by three orders of magnitude.
    ///
    /// Stated here in Rust as well as in the SQL pattern because the two have to agree,
    /// and only one of them is checked by the compiler.
    #[test]
    fn a_clone_name_is_the_prefix_and_exactly_32_hex_digits() {
        let name = process_db_name();
        let hex = name
            .strip_prefix("egret_t_")
            .expect("clone names start with the prefix the sweep anchors on");
        assert_eq!(hex.len(), 32, "{name}");
        assert!(
            hex.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "{name} must be lowercase hex — the sweep's character class is [0-9a-f]"
        );

        // And the shape of the bug this replaced. `egret_t_` is not literally a prefix
        // of `egret_test` at all — the eighth character is `e`, not `_`. `LIKE
        // 'egret_t_%'` matched anyway because `_` is a single-character wildcard there,
        // so it consumed that `e`. A regex `_` is literal, which is the whole reason the
        // sweep uses one.
        assert_eq!("egret_test".strip_prefix("egret_t_"), None);
    }

    /// The same claim, asked of Postgres — because it is Postgres' regex engine that
    /// decides, not Rust's reading of the string. Self-skips without `DATABASE_URL` ONLY
    /// under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.
    ///
    /// Deliberately does NOT go through [`super::url`]: it needs no clone, only an
    /// evaluation of the pattern, and cloning here would make the test that guards the
    /// sweep depend on the machinery the sweep protects.
    #[tokio::test]
    async fn postgres_matches_a_clone_and_never_the_shared_database() {
        use sqlx::Connection;
        // Held to the same rule this crate now imposes on everyone else: a run with no
        // database FAILS unless the operator asked for a skip. This test guards the sweep
        // that can drop a database, so "it went green" must never mean "it did not run".
        let url = match std::env::var("DATABASE_URL") {
            Ok(u) => u,
            Err(_) => {
                if let Err(msg) = super::skip_verdict(false, super::allow_skip()) {
                    panic!("{msg}");
                }
                eprintln!("skip: DATABASE_URL unset ({ALLOW_SKIP_VAR} is set)");
                return;
            }
        };
        let mut conn = match sqlx::PgConnection::connect(&url).await {
            Ok(c) => c,
            Err(e) => {
                if let Err(msg) = super::skip_verdict(true, super::allow_skip()) {
                    panic!("{msg}\n\nunderlying connect error: {e}");
                }
                eprintln!("skip: no database ({ALLOW_SKIP_VAR} is set)");
                return;
            }
        };
        let mine = process_db_name();
        let cases: [(&str, bool); 7] = [
            (mine.as_str(), true),
            ("egret_test", false),
            ("egret", false),
            ("egret_test_template", false),
            // 16 hex digits is not 32.
            ("egret_t_0123456789abcdef", false),
            // Anchored at both ends.
            ("xegret_t_00000000000000000000000000000001", false),
            ("egret_t_00000000000000000000000000000001x", false),
        ];
        for (name, expected) in cases {
            let (matched,): (bool,) = sqlx::query_as("SELECT $1 ~ $2")
                .bind(name)
                .bind(CLONE_PATTERN)
                .fetch_one(&mut conn)
                .await
                .expect("evaluate the pattern");
            assert_eq!(matched, expected, "{name} against {CLONE_PATTERN}");
        }
    }
}
