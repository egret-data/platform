//! SP2b Task 22: the evaluator's cost per OPA question, on generated snapshots.
//!
//! ```bash
//! cargo bench -p egret-perm-eval -- --save-baseline sp2b
//! ```
//!
//! The generator lays out 100 catalogs × 10 schemas × 10 tables (`c000.s0.t0`
//! … `c099.s9.t9`) and takes the first `tables` of them in that order, so the
//! 1 000-table snapshot covers catalogs `c000`–`c009` and the 10 000-table one
//! covers all 100. On every table: one Read grant to `ann` and one other user.
//! On 5 % of the tables (every 20th): a `MASK_HASH` on `email` and a row
//! filter. Plus one deny and the Trino infrastructure set (`lower_infra`).
//! The statement count of each snapshot is printed before its benches run.
//! The bench IDs' `/1000` and `/10000` are those TABLE counts (1 106 and 11 006
//! statements).
//!
//! Every bench asks about objects inside `c000`–`c009`, so both sizes answer
//! the same question; what differs is how many statements the snapshot holds.
//!
//! The snapshot indexes path statements by catalog only, so on that grid a
//! question meets about 110 candidate statements whatever the size. The
//! `*_one_catalog` benches put the same tables in ONE catalog (`c000`,
//! schemas `s000`…, ten tables each): every statement is then a candidate,
//! which is the shape of an installation with one big catalog.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use egret_perm::EngineType;
use egret_perm_eval::{Decision, Resource, Snapshot, SnapshotInput};
use egret_perm_ir::audience::{Audience, Member, MemberName, Via};
use egret_perm_ir::lower::infra::{lower_infra, InfraInputs};
use egret_perm_ir::statement::*;
use std::hint::black_box;
use uuid::Uuid;

const SIZES: [usize; 2] = [1_000, 10_000];

fn user(name: &str) -> Member {
    Member {
        name: MemberName::User(name.into()),
        via: Via::Direct,
    }
}

fn users(names: &[&str]) -> Audience {
    Audience {
        include: names.iter().map(|n| user(n)).collect(),
        except: Vec::new(),
    }
}

fn path(c: &str, s: &str, t: Option<&str>, col: Option<&str>) -> Target {
    let l = |v: Option<&str>| v.map_or(Level::Unnamed, |v| Level::Named(v.into()));
    Target::Path(Path {
        catalog_id: None,
        catalog: c.into(),
        schema: Level::Named(s.into()),
        table: l(t),
        column: l(col),
    })
}

fn statement(source: Source, audience: Audience, target: Target, effect: Effect) -> Statement {
    Statement {
        source,
        audience,
        target,
        effect,
        derivation: Derivation::AsDeclared,
    }
}

fn grant(n: u128) -> Source {
    Source::Grant {
        grant_id: Uuid::from_u128(n),
    }
}

fn rule(n: u128, name: String) -> Source {
    Source::Rule {
        rule_id: Uuid::from_u128(n),
        rule_name: name,
    }
}

#[derive(Clone, Copy)]
enum Layout {
    /// 100 catalogs × 10 schemas × 10 tables.
    Grid,
    /// One catalog, 10 tables per schema.
    OneCatalog,
}

/// `(catalog, schema, table)` of the `i`-th table.
fn table_at(layout: Layout, i: usize) -> (String, String, String) {
    match layout {
        Layout::Grid => (
            format!("c{:03}", i / 100),
            format!("s{}", (i / 10) % 10),
            format!("t{}", i % 10),
        ),
        Layout::OneCatalog => (
            "c000".to_string(),
            format!("s{:03}", i / 10),
            format!("t{}", i % 10),
        ),
    }
}

fn statements(layout: Layout, tables: usize) -> Vec<Statement> {
    let mut out = Vec::new();
    let mut n: u128 = 1;
    for i in 0..tables {
        let (c, s, t) = table_at(layout, i);
        let other = format!("u{}", i % 100);
        out.push(statement(
            grant(n),
            users(&["ann", &other]),
            path(&c, &s, Some(&t), None),
            Effect::Allow {
                privileges: vec![Privilege::Read],
            },
        ));
        n += 1;
        if i % 20 == 0 {
            out.push(statement(
                rule(n, format!("mask_{i}")),
                users(&["ann", &other]),
                path(&c, &s, Some(&t), Some("email")),
                Effect::Mask {
                    mask_type: "MASK_HASH".into(),
                    expression: None,
                },
            ));
            n += 1;
            out.push(statement(
                rule(n, format!("filter_{i}")),
                users(&["ann", &other]),
                path(&c, &s, Some(&t), None),
                Effect::RowFilter {
                    expression: "region = 'us'".into(),
                },
            ));
            n += 1;
        }
    }
    out.push(statement(
        grant(n),
        users(&["u1"]),
        path("c000", "s9x", None, None),
        Effect::Deny {
            privileges: vec![Privilege::Read],
        },
    ));
    out.extend(lower_infra(&InfraInputs {
        engine: EngineType::Trino,
        admins: vec!["admin".into()],
        delegating: true,
        delegator_impersonates: true,
    }));
    out
}

fn snapshot(layout: Layout, tables: usize) -> Snapshot {
    Snapshot::build(SnapshotInput {
        service_id: 1,
        key: (1, 1),
        statements: statements(layout, tables),
        classified: Vec::new(),
        lineage_principal: "egret-lineage".into(),
    })
    .expect("the generated snapshot builds")
}

fn table(layout: Layout, i: usize, columns: Vec<String>) -> Resource {
    let (catalog, schema, table) = table_at(layout, i);
    Resource::Table {
        catalog,
        schema,
        table,
        columns,
    }
}

fn benches(c: &mut Criterion) {
    for (layout, suffix) in [(Layout::Grid, ""), (Layout::OneCatalog, "_one_catalog")] {
        run(c, layout, suffix);
    }
}

fn run(c: &mut Criterion, layout: Layout, suffix: &str) {
    let snaps: Vec<(usize, Snapshot)> = SIZES.iter().map(|&n| (n, snapshot(layout, n))).collect();
    for (n, s) in &snaps {
        eprintln!(
            "snapshot layout={suffix:?} tables={n} statements={}",
            s.len()
        );
    }

    let mut g = c.benchmark_group("build");
    for &n in &SIZES {
        g.bench_with_input(
            BenchmarkId::new(format!("snapshot{suffix}"), n),
            &n,
            |b, &n| {
                b.iter_batched(
                    || statements(layout, n),
                    |st| {
                        Snapshot::build(SnapshotInput {
                            service_id: 1,
                            key: (1, 1),
                            statements: st,
                            classified: Vec::new(),
                            lineage_principal: "egret-lineage".into(),
                        })
                        .expect("builds")
                    },
                    criterion::BatchSize::LargeInput,
                )
            },
        );
    }
    g.finish();

    // A masked, filtered table (index 980 is a multiple of 20).
    let eight: Vec<String> = ["id", "email", "region", "name", "a", "b", "c", "d"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let select = table(layout, 980, eight);
    let mut g = c.benchmark_group("decide");
    for (n, s) in &snaps {
        let d = s.decide("ann", "SelectFromColumns", &select);
        assert!(matches!(d, Decision::Allow { .. }), "{d:?}");
        g.bench_with_input(
            BenchmarkId::new(format!("select_8_columns{suffix}"), n),
            s,
            |b, s| b.iter(|| s.decide(black_box("ann"), "SelectFromColumns", black_box(&select))),
        );
    }
    g.finish();

    let tables: Vec<Resource> = (0..1_000).map(|i| table(layout, i, Vec::new())).collect();
    let mut g = c.benchmark_group("filter");
    for (n, s) in &snaps {
        let allowed = s.filter("ann", "FilterTables", &tables).len();
        assert_eq!(
            allowed, 1_000,
            "ann reads every one of the first 1 000 tables"
        );
        g.bench_with_input(
            BenchmarkId::new(format!("FilterTables_1000{suffix}"), n),
            s,
            |b, s| b.iter(|| s.filter(black_box("ann"), "FilterTables", black_box(&tables))),
        );
    }
    g.finish();

    let mut columns: Vec<(String, String)> = (0..499)
        .map(|i| (format!("col{i}"), "varchar".to_string()))
        .collect();
    columns.push(("email".into(), "varchar".into()));
    let (cat, sch, tab) = table_at(layout, 980);
    let mut g = c.benchmark_group("column_masks");
    for (n, s) in &snaps {
        let masks = s
            .column_masks("ann", &cat, &sch, &tab, &columns)
            .expect("masks");
        assert_eq!(masks.len(), 1, "only email is masked: {masks:?}");
        g.bench_with_input(
            BenchmarkId::new(format!("500_columns{suffix}"), n),
            s,
            |b, s| {
                b.iter(|| {
                    s.column_masks(black_box("ann"), &cat, &sch, &tab, black_box(&columns))
                        .expect("masks")
                })
            },
        );
    }
    g.finish();
}

criterion_group!(eval, benches);
criterion_main!(eval);
