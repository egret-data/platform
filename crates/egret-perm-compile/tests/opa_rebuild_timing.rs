//! SP2b Task 22 (follow-up T7 M1): how long the OPA refresher takes to rebuild
//! one profile's snapshot when its key moves, at about 1 000 and 10 000
//! statements, and how much of that is the stored-overlap scan
//! ([`stored_overlaps_via`]), which reads EVERY service's stored units on each
//! rebuild.
//!
//! `#[ignore]`d: it writes about 20 000 grants and measures. Release only, alone:
//!
//! ```bash
//! env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!   cargo test -p egret-perm-compile --release --features harness --test opa_rebuild_timing \
//!   -- --ignored --nocapture
//! ```
//!
//! Shape of one profile: a Trino profile with a tag service and an engine
//! bound to it; one catalog; `tables` per-table Read grants to one user
//! (`s000.t0` …, ten tables a schema); on every 20th table a path `MASK_HASH`
//! on `email` and a path row filter.
//!
//! **The 1 000-policy ceiling.** `recompile_service` refuses a profile over
//! `PROFILE_EXPANSION_CEILING` (1 000 compiler-owned policies), so a
//! compiled profile here has 820 tables: 820 grants plus 41 masks and 41
//! filters, which compile to 984 policies (measured: 900 tables gave 1 080,
//! over the ceiling). The 10 000-table profile (11 000 model rows) cannot be
//! compiled; it is written and switched to OPA without a compile, which the
//! snapshot does not need (it lowers the model rows). Its key is moved by hand.
//!
//! Steps, each printing one line:
//!
//! 1. `1k_alone`: one compiled 820-table profile, alone in the database;
//! 2. `10k_uncompiled`: the 10 000-table profile added;
//! 3. nine more compiled 820-table profiles (Ranger; about 8 900 more stored
//!    policies), then `1k_beside_10_profiles` and `10k_beside_10_profiles`:
//!    the same two rebuilds while the overlap scan has every service's
//!    stored units to read.
//!
//! Each step: `RUNS` times `policy_version + 1` then `refresh_once` (asserting
//! the profile was rebuilt), and `RUNS` times `stored_overlaps_via` alone in a
//! `REPEATABLE READ, READ ONLY` transaction, as a build ran it before Task 22F.
//!
//! Task 22F: a build now runs `stored_overlaps_of_via`, which reads only the
//! profile's own units; each step also times it alone (`overlap_scan_profile`),
//! prints every rebuild sample in run order, and prints how many rows of
//! `policy.authz_policies` changed since Postgres last analyzed it. A last step,
//! `1k_beside_10_profiles_again`, repeats the 1k measurement after the 10k one,
//! to tell the profile's shape from the order the steps ran in (Task 22's
//! unexplained 1k-slower-than-10k result).
//!
//! Task 26 (Task 3), a second test, `opa_shared_tag_service_refresh_latency`
//! (run it alone with `--exact opa_shared_tag_service_refresh_latency`): N
//! compiled 820-table profiles (984 stored policies each) bound to ONE tag
//! service, N = 1, 5, 10, 20; the tag version moves on all of them in one
//! statement, then `refresh_once` is driven directly while a thread polls the
//! cache every 100 µs for the moment each profile is served a snapshot at its
//! new key. Ten runs per N; it prints the first and the last profile's
//! moment, each rebuild, the scoped overlap scan, and two pieces of one build
//! (`stored_state_refusals_via`, `snapshot_parts_via`).
include!("opa_support/fixtures_host.rs");
#[allow(dead_code)]
#[path = "golden/fixtures.rs"]
mod fixtures;

use std::time::{Duration, Instant};

use egret_metastore::Db;
use egret_perm::{Action, Effect, PermStore};
use egret_perm_compile::opa::cache::{Lookup, SnapshotCache};
use egret_perm_compile::overlap::{stored_overlaps_of_via, stored_overlaps_via};
use egret_perm_compile::recompile_service;
use egret_policy::PolicyStore;
use fixtures::*;
use serde_json::json;
use uuid::Uuid;

const RUNS: usize = 10;

async fn db() -> Db {
    let dsn = egret_test_db::url().await.expect("a test database");
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    seed_service_defs(&db).await;
    db
}

struct Profile {
    id: i64,
    tables: usize,
}

async fn arrange(
    db: &Db,
    prefix: &str,
    tables: usize,
    compile: bool,
    opa: bool,
    names: &mut Names,
) -> Profile {
    arrange_with(db, prefix, tables, compile, opa, None, names).await
}

/// [`arrange`], with `config` (e.g. a shared `tagService`) in place of the
/// profile's own tag service when Some.
async fn arrange_with(
    db: &Db,
    prefix: &str,
    tables: usize,
    compile: bool,
    opa: bool,
    config: Option<serde_json::Value>,
    names: &mut Names,
) -> Profile {
    let profile = match config {
        Some(cfg) => profile_with_config(db, prefix, "trino", cfg, names).await,
        None => fixtures::profile(db, prefix, "trino", true, names).await.0,
    };
    let mut tx = db.pool.begin().await.expect("tx");
    let wh = warehouse(&mut tx, prefix, names).await;
    let engine = engine_bound_spec(
        &mut tx,
        &format!("{prefix}.engine"),
        profile,
        "trino",
        json!({"trinoConfig": {"delegationMode": "open"}}),
        names,
    )
    .await;
    let cat: Uuid = catalog(&mut tx, &format!("{prefix}.lake"), wh, &[engine], names).await;
    let ann = format!("{prefix}_ann");
    user(&mut tx, &format!("{prefix}.ann"), Some(&ann), names).await;
    for i in 0..tables {
        let schema = format!("s{:03}", i / 10);
        let table = format!("t{}", i % 10);
        grant(
            &mut tx,
            &format!("{prefix}.g{i}"),
            Effect::Allow,
            &[Action::Read],
            scope(cat, Some(&schema), Some(&table), None),
            set(&[user_ref(&ann)], &[]),
            names,
        )
        .await;
        if i % 20 == 0 {
            rule(
                &mut tx,
                &format!("{prefix}_mask_{i}"),
                mask("MASK_HASH", None),
                egret_perm::Selector::Path(scope(cat, Some(&schema), Some(&table), Some("email"))),
                set(&[], &[]),
                names,
            )
            .await;
            rule(
                &mut tx,
                &format!("{prefix}_filter_{i}"),
                row_filter("region = 'us'"),
                egret_perm::Selector::Path(scope(cat, Some(&schema), Some(&table), None)),
                set(&[], &[]),
                names,
            )
            .await;
        }
    }
    tx.commit().await.expect("commit");
    if compile {
        let t = Instant::now();
        recompile_service(
            &PermStore::new(db.pool.clone()),
            &PolicyStore::new(db.clone()),
            profile,
        )
        .await
        .expect("recompile");
        let stored: i64 =
            sqlx::query_scalar("SELECT count(*) FROM policy.authz_policies WHERE service_id = $1")
                .bind(profile)
                .fetch_one(&db.pool)
                .await
                .expect("stored");
        println!(
            "arrange prefix={prefix} tables={tables} recompile={:.3}s stored_policies={stored}",
            t.elapsed().as_secs_f64()
        );
    } else {
        println!("arrange prefix={prefix} tables={tables} not compiled");
    }
    if opa {
        sqlx::query("UPDATE policy.authz_services SET mechanism = 'opa' WHERE id = $1")
            .bind(profile)
            .execute(&db.pool)
            .await
            .expect("mechanism");
    }
    Profile {
        id: profile,
        tables,
    }
}

fn line(xs: &mut [Duration]) -> String {
    xs.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    let p50 = ms(xs[xs.len() / 2]);
    let mean = xs.iter().map(|d| ms(*d)).sum::<f64>() / xs.len() as f64;
    format!(
        "n={} min={:.1}ms p50={:.1}ms max={:.1}ms mean={:.1}ms",
        xs.len(),
        ms(xs[0]),
        p50,
        ms(xs[xs.len() - 1]),
        mean
    )
}

async fn services(db: &Db) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM policy.authz_services")
        .fetch_one(&db.pool)
        .await
        .expect("count")
}

/// `policy.authz_policies`'s rows changed since its last analyze, and when
/// that analyze ran.
async fn analyze_state(db: &Db) -> String {
    let (changed, auto, manual): (i64, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT n_mod_since_analyze, last_autoanalyze::text, last_analyze::text \
           FROM pg_stat_user_tables WHERE schemaname = 'policy' AND relname = 'authz_policies'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("pg_stat_user_tables");
    format!("n_mod_since_analyze={changed} last_autoanalyze={auto:?} last_analyze={manual:?}")
}

async fn measure(db: &Db, label: &str, p: &Profile, cache: &SnapshotCache) {
    println!("{label} before: {}", analyze_state(db).await);
    let mut rebuild = Vec::new();
    for _ in 0..RUNS {
        sqlx::query(
            "UPDATE policy.authz_services SET policy_version = policy_version + 1 WHERE id = $1",
        )
        .bind(p.id)
        .execute(&db.pool)
        .await
        .expect("bump");
        let t = Instant::now();
        let report = cache.refresh_once(&db.pool).await.expect("refresh");
        rebuild.push(t.elapsed());
        assert!(report.rebuilt.contains(&p.id), "{report:?}");
    }
    let samples: Vec<String> = rebuild
        .iter()
        .map(|d| format!("{:.1}", d.as_secs_f64() * 1e3))
        .collect();
    println!(
        "{label} rebuild samples in run order (ms): {}",
        samples.join(" ")
    );
    let statements = match cache.get(p.id) {
        Lookup::Ready(s) => s.len(),
        other => panic!("not ready: {other:?}"),
    };
    let mut scan = Vec::new();
    let mut found = 0;
    for _ in 0..RUNS {
        let mut tx = db.pool.begin().await.expect("tx");
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await
            .expect("iso");
        let t = Instant::now();
        found = stored_overlaps_via(&mut tx).await.expect("scan").len();
        scan.push(t.elapsed());
    }
    let mut scoped = Vec::new();
    let mut found_scoped = 0;
    for _ in 0..RUNS {
        let mut tx = db.pool.begin().await.expect("tx");
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await
            .expect("iso");
        let t = Instant::now();
        found_scoped = stored_overlaps_of_via(&mut tx, p.id)
            .await
            .expect("scoped scan")
            .len();
        scoped.push(t.elapsed());
    }
    println!(
        "{label} tables={} statements={statements} services_in_db={} rebuild {} | overlap_scan {} (pairs={found}) | overlap_scan_profile {} (pairs={found_scoped})",
        p.tables,
        services(db).await,
        line(&mut rebuild),
        line(&mut scan),
        line(&mut scoped),
    );
    println!("{label} after: {}", analyze_state(db).await);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "measurement; release only, see the module doc"]
async fn opa_snapshot_rebuild_time_at_1k_and_10k_statements() {
    let db = db().await;
    let mut names = Names::default();
    let cache = SnapshotCache::default();

    let small = arrange(&db, "rt1k", 820, true, true, &mut names).await;
    let t = Instant::now();
    let report = cache.refresh_once(&db.pool).await.expect("refresh");
    assert!(report.rebuilt.contains(&small.id), "{report:?}");
    println!(
        "initial rt1k: first refresh_once {:.1}ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    measure(&db, "1k_alone", &small, &cache).await;

    let large = arrange(&db, "rt10k", 10_000, false, true, &mut names).await;
    let t = Instant::now();
    let report = cache.refresh_once(&db.pool).await.expect("refresh");
    assert!(report.rebuilt.contains(&large.id), "{report:?}");
    println!(
        "initial rt10k: first refresh_once {:.1}ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    measure(&db, "10k_uncompiled", &large, &cache).await;

    for i in 0..9 {
        arrange(&db, &format!("rtf{i}"), 820, true, false, &mut names).await;
    }
    measure(&db, "1k_beside_10_profiles", &small, &cache).await;
    measure(&db, "10k_beside_10_profiles", &large, &cache).await;
    measure(&db, "1k_beside_10_profiles_again", &small, &cache).await;
}

// ── Task 26 (Task 3): N profiles sharing one tag service ────────────────────

/// The N values Task 26 Task 3 measures.
const SHARED_NS: [usize; 4] = [1, 5, 10, 20];

/// Each of `ids`' first moment (from `t0`) at which the cache serves it a
/// snapshot at its key in `target`, polled every 100 µs on its own thread.
fn watch_current(
    cache: std::sync::Arc<SnapshotCache>,
    target: Vec<(i64, (i64, i64))>,
    t0: Instant,
) -> std::thread::JoinHandle<Vec<(i64, Duration)>> {
    std::thread::spawn(move || {
        let mut done: Vec<Option<Duration>> = vec![None; target.len()];
        let deadline = t0 + Duration::from_secs(120);
        while done.iter().any(Option::is_none) {
            assert!(Instant::now() < deadline, "not current in 120 s");
            for (i, (id, key)) in target.iter().enumerate() {
                if done[i].is_some() {
                    continue;
                }
                if let Lookup::Ready(s) = cache.get(*id) {
                    if s.key() == *key {
                        done[i] = Some(t0.elapsed());
                    }
                }
            }
            std::thread::sleep(Duration::from_micros(100));
        }
        target
            .iter()
            .zip(done)
            .map(|((id, _), d)| (*id, d.expect("current")))
            .collect()
    })
}

async fn measure_shared(db: &Db, tag: &str, ids: &[i64], cache: &std::sync::Arc<SnapshotCache>) {
    let n = ids.len();
    let label = format!("shared_tag_n{n}");
    println!("{label} before: {}", analyze_state(db).await);
    let (mut last, mut first, mut whole, mut each) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for run in 0..RUNS {
        // The tag version moves on every profile naming the tag service and on
        // the tag service itself, in one statement, as
        // `egret_catalog_registry::tag_version` moves it.
        let mut target: Vec<(i64, (i64, i64))> = sqlx::query_as(
            "UPDATE policy.authz_services SET tag_version = tag_version + 1, updated_at = now() \
              WHERE id = ANY($1) OR (name = $2 AND type = 'tag') \
              RETURNING id, policy_version, tag_version",
        )
        .bind(ids)
        .bind(tag)
        .fetch_all(&db.pool)
        .await
        .expect("bump")
        .into_iter()
        .filter(|(id, _, _): &(i64, i64, i64)| ids.contains(id))
        .map(|(id, pv, tv)| (id, (pv, tv)))
        .collect();
        target.sort_unstable();
        assert_eq!(target.len(), n);
        let t0 = Instant::now();
        let watcher = watch_current(std::sync::Arc::clone(cache), target, t0);
        let report = cache.refresh_once(&db.pool).await.expect("refresh");
        let refresh = t0.elapsed();
        for id in ids {
            assert!(report.rebuilt.contains(id), "{report:?}");
        }
        let mut done = watcher.join().expect("watcher");
        done.sort_by_key(|(_, d)| *d);
        let times: Vec<String> = done
            .iter()
            .map(|(_, d)| format!("{:.1}", d.as_secs_f64() * 1e3))
            .collect();
        println!(
            "{label} run={run} refresh_once={:.1}ms current_at(ms, in order)={}",
            refresh.as_secs_f64() * 1e3,
            times.join(" ")
        );
        first.push(done[0].1);
        last.push(done[n - 1].1);
        whole.push(refresh);
        let mut prev = Duration::ZERO;
        for (_, d) in &done {
            each.push(*d - prev);
            prev = *d;
        }
    }
    let mut scoped = Vec::new();
    let mut pairs = 0;
    for _ in 0..RUNS {
        let mut tx = db.pool.begin().await.expect("tx");
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await
            .expect("iso");
        let t = Instant::now();
        pairs = stored_overlaps_of_via(&mut tx, ids[0])
            .await
            .expect("scoped scan")
            .len();
        scoped.push(t.elapsed());
    }
    // What one build of ids[0] spends, piece by piece, each alone in its own
    // `REPEATABLE READ, READ ONLY` transaction, as a build runs them.
    let (mut guard, mut parts, mut reads) = (Vec::new(), Vec::new(), Vec::new());
    let mut model_rows = 0;
    for _ in 0..RUNS {
        let mut tx = db.pool.begin().await.expect("tx");
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await
            .expect("iso");
        let t = Instant::now();
        egret_policy::opa_guard::stored_state_refusals_via(&mut tx, ids[0])
            .await
            .expect("guard");
        guard.push(t.elapsed());
        let t = Instant::now();
        egret_perm_compile::opa::build::snapshot_parts_via(
            &mut tx,
            ids[0],
            egret_perm_compile::opa::build::MissingUsername::Omit,
        )
        .await
        .expect("parts")
        .expect("trino");
        parts.push(t.elapsed());
        // The model reads `ir_load::load_ep` makes for ids[0], alone: what
        // grows with the installation, not with the profile.
        let t = Instant::now();
        let (grants, _rules) =
            egret_perm_compile::harness_support::load_ep_model_ids_for_test(&mut tx, ids[0])
                .await
                .expect("load_ep")
                .expect("trino");
        reads.push(t.elapsed());
        model_rows = grants.len();
    }
    println!(
        "{label} pieces: stored_state_refusals {} | snapshot_parts {} | load_ep {} (grants read={model_rows})",
        line(&mut guard),
        line(&mut parts),
        line(&mut reads),
    );
    println!(
        "{label} profiles={n} services_in_db={} last_current {} | first_current {} | refresh_once {} | one_rebuild {} | overlap_scan_profile {} (pairs={pairs})",
        services(db).await,
        line(&mut last),
        line(&mut first),
        line(&mut whole),
        line(&mut each),
        line(&mut scoped),
    );
    println!("{label} after: {}", analyze_state(db).await);
}

/// Task 26 Task 3: N compiled profiles (984 stored policies each) bound to ONE
/// tag service, all on OPA; the tag version moves on all of them at once; how
/// long until the LAST one is served a snapshot at its new key. `refresh_once`
/// is driven directly (no 1 s ticker); see the report for the period's share.
/// The profiles accumulate (1, then 4 more, ...), and `ANALYZE` runs after
/// each arrange so no step measures on never-analyzed statistics (22F.3).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "measurement; release only, see the module doc"]
async fn opa_shared_tag_service_refresh_latency() {
    let db = db().await;
    let mut names = Names::default();
    let cache = std::sync::Arc::new(SnapshotCache::default());
    let tag = "st_shared_tag";
    PolicyStore::new(db.clone())
        .create_service(tag, "tag", &json!({}))
        .await
        .expect("tag service");
    let mut ids: Vec<i64> = Vec::new();
    for n in SHARED_NS {
        while ids.len() < n {
            let p = arrange_with(
                &db,
                &format!("st{}", ids.len()),
                820,
                true,
                true,
                Some(json!({ "tagService": tag })),
                &mut names,
            )
            .await;
            ids.push(p.id);
        }
        sqlx::query("ANALYZE")
            .execute(&db.pool)
            .await
            .expect("analyze");
        let t = Instant::now();
        let report = cache.refresh_once(&db.pool).await.expect("refresh");
        println!(
            "initial shared_tag_n{n}: refresh_once {:.1}ms rebuilt={}",
            t.elapsed().as_secs_f64() * 1e3,
            report.rebuilt.len()
        );
        for id in &ids {
            assert!(matches!(cache.get(*id), Lookup::Ready(_)), "{id} not ready");
        }
        measure_shared(&db, tag, &ids, &cache).await;
    }
}
