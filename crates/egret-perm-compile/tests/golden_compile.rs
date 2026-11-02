//! Golden corpus for governance sub-project 1: the Ranger artifacts every
//! fixture profile compiles to, captured on unchanged code and compared byte
//! for byte after every refactor task. Bless with EGRET_GOLDEN_BLESS=1.
//!   DATABASE_URL=postgres://egret:egret@localhost:5432/egret \
//!   cargo test -p egret-perm-compile --features harness --test golden_compile
#[path = "golden/fixtures.rs"]
mod fixtures;

use std::collections::BTreeMap;
use std::path::PathBuf;

use egret_metastore::Db;
use serde_json::{json, Value};
use uuid::Uuid;

/// Ids the fixture created, by role name; anything else in the output fails.
#[derive(Default)]
pub struct Names(pub BTreeMap<String, String>);

impl Names {
    pub fn id(&mut self, role: &str, id: Uuid) -> Uuid {
        self.0.insert(id.to_string(), format!("{{{role}}}"));
        id
    }
    pub fn text(&mut self, role: &str, value: &str) {
        self.0.insert(value.to_string(), format!("{{{role}}}"));
    }
}

fn canonical(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let sorted: BTreeMap<_, _> = m.iter().map(|(k, v)| (k.clone(), canonical(v))).collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

fn normalise(text: String, names: &Names) -> String {
    let mut out = text;
    // Longest first, so a username that is a prefix of another is not split.
    let mut keys: Vec<&String> = names.0.keys().collect();
    keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
    for k in keys {
        out = out.replace(k.as_str(), &names.0[k]);
    }
    out
}

fn assert_no_unregistered_uuid(fixture: &str, text: &str) {
    let re = |s: &str| {
        s.split(|c: char| !(c.is_ascii_hexdigit() || c == '-'))
            .find(|w| w.len() == 36 && Uuid::parse_str(w).is_ok())
            .map(str::to_string)
    };
    if let Some(u) = re(text) {
        panic!(
            "{fixture}: unregistered uuid {u} in the golden output — register it in the fixture"
        );
    }
}

pub async fn capture(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    profiles: &[(i64, Option<i64>)],
) -> Value {
    let mut out = Vec::new();
    for &(sid, tag_sid) in profiles {
        let compile = egret_perm_compile::recompile_service_in_tx(tx, sid).await;
        let mut entry = serde_json::Map::new();
        match compile {
            Err(e) => {
                entry.insert("error".into(), json!(e.to_string()));
                // A refusal after the compile (the profile-total ceiling, in
                // `apply_diff_in_tx`) still has a desired set behind it; too
                // large to store whole (G21), it is pinned by count and digest.
                if let Ok(Some(g)) =
                    egret_perm_compile::harness_support::golden_desired_in_tx(tx, sid).await
                {
                    let all = json!(g
                        .desired
                        .iter()
                        .map(|(s, n, b)| json!({"service": s, "name": n, "body": canonical(b)}))
                        .collect::<Vec<_>>());
                    use sha2::Digest;
                    let digest = sha2::Sha256::digest(all.to_string().as_bytes());
                    entry.insert(
                        "desiredBehindRefusal".into(),
                        json!({"count": g.desired.len(), "sha256": hex::encode(digest), "skipped": g.skipped.len()}),
                    );
                }
            }
            Ok(summary) => {
                let g = egret_perm_compile::harness_support::golden_desired_in_tx(tx, sid)
                    .await
                    .expect("golden desired")
                    .expect("a compiler-targeted profile");
                entry.insert(
                    "desired".into(),
                    json!(g
                        .desired
                        .iter()
                        .map(|(s, n, b)| json!({"service": s, "name": n, "body": canonical(b)}))
                        .collect::<Vec<_>>()),
                );
                entry.insert(
                    "skipped".into(),
                    json!(summary
                        .skipped
                        .iter()
                        .map(|s| json!({"kind": format!("{:?}", s.source_kind), "id": s.id, "reason": s.reason}))
                        .collect::<Vec<_>>()),
                );
                for (key, id) in [("served", Some(sid)), ("servedTag", tag_sid)] {
                    let Some(id) = id else { continue };
                    let rows = egret_policy::enabled_policies_for_service_via(tx, id)
                        .await
                        .expect("served");
                    let rows: Vec<Value> = rows
                        .into_iter()
                        .enumerate()
                        .map(|(i, mut b)| {
                            b["id"] = json!(i); // DB sequence → ordinal; order is kept
                            canonical(&b)
                        })
                        .collect();
                    entry.insert(key.into(), json!(rows));
                }
            }
        }
        out.push(Value::Object(entry));
    }
    json!(out)
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

pub fn compare_or_bless(fixture: &str, captured: Value, names: &Names) {
    let text = normalise(serde_json::to_string_pretty(&captured).unwrap(), names);
    assert_no_unregistered_uuid(fixture, &text);
    let path = golden_dir().join(format!("{fixture}.json"));
    if std::env::var_os("EGRET_GOLDEN_BLESS").is_some() {
        std::fs::write(&path, format!("{text}\n")).expect("bless");
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{fixture}: no golden at {path:?}; bless it on unchanged code"));
    if want.trim_end() != text {
        let actual = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/golden-actual")
            .join(format!("{fixture}.json"));
        std::fs::create_dir_all(actual.parent().unwrap()).ok();
        std::fs::write(&actual, &text).ok();
        panic!("{fixture}: compiled artifacts differ from the golden; diff {path:?} {actual:?}");
    }
}

async fn golden_db() -> Db {
    let dsn = egret_test_db::url().await.expect("a test database");
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    fixtures::seed_service_defs(&db).await;
    db
}

#[tokio::test]
async fn every_golden_fixture_compiles_to_its_golden_bytes() {
    let db = golden_db().await;
    let mut ran = 0;
    for f in fixtures::ALL {
        eprintln!("golden fixture {}", f.name);
        (f.run)(&db).await;
        ran += 1;
    }
    assert_eq!(ran, fixtures::ALL.len());
    assert!(ran >= 21, "the corpus shrank: {ran} fixtures");
}

/// The Task 11 performance baseline: G21's profile compiled 7 times, the
/// median printed. Release mode:
///   cargo test -p egret-perm-compile --release --features harness \
///     --test golden_compile -- --ignored --nocapture g21_compile_timing
#[tokio::test]
#[ignore]
async fn g21_compile_timing() {
    let db = golden_db().await;
    let mut names = Names::default();
    let (sid, mut tx) = fixtures::g21_build(&db, &mut names).await;
    let mut times = Vec::new();
    for _ in 0..7 {
        let start = std::time::Instant::now();
        let g = egret_perm_compile::harness_support::golden_desired_in_tx(&mut tx, sid)
            .await
            .expect("compile")
            .expect("a compiler-targeted profile");
        times.push(start.elapsed().as_secs_f64());
        assert!(!g.desired.is_empty());
    }
    tx.rollback().await.expect("rollback");
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "g21_compile_timing: median={:.3}s runs={times:?}",
        times[times.len() / 2]
    );
}
