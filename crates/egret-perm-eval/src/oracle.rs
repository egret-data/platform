//! Task 22F: the linear scan, kept as a test-only oracle for the path index.
//!
//! With the oracle on, `Snapshot::candidates` returns every statement, so each
//! caller tests every statement's target in full: the evaluator as it would be
//! with no index at all. The randomized test below asks the same questions of
//! generated snapshots with the index and with the oracle, and requires the
//! same answers, reasons and errors included. A statement the index drops that
//! should match (a deny, a withhold, a mask or a filter) shows up as a
//! difference.

use std::cell::Cell;

thread_local! {
    static LINEAR: Cell<bool> = const { Cell::new(false) };
}

/// Whether `candidates` scans every statement (the oracle) on this thread.
pub(crate) fn linear() -> bool {
    LINEAR.with(Cell::get)
}

/// Runs `f` with the oracle on, on this thread, and turns it off again.
fn with_linear<R>(f: impl FnOnce() -> R) -> R {
    struct Off;
    impl Drop for Off {
        fn drop(&mut self) {
            LINEAR.with(|l| l.set(false));
        }
    }
    LINEAR.with(|l| l.set(true));
    let _off = Off;
    f()
}

mod equivalence {
    use egret_perm_ir::audience::{Audience, Member, MemberName, Via};
    use egret_perm_ir::statement::{
        Derivation, Effect, InfraKind, InfraObject, Level, Path, Privilege, Source, Statement,
        Target,
    };

    use super::with_linear;
    use crate::decide::{Decision, ObjectPath, Resource};
    use crate::snapshot::{ClassifiedColumn, Snapshot, SnapshotInput};
    use crate::trino_ops::{DataPrivilege, OpClass, TRINO_483_OPERATIONS};

    const LINEAGE: &str = "egret-lineage";
    const USERS: &[&str] = &["ann", "bob", LINEAGE];

    /// Statement values: literals in several cases, wildcard patterns and
    /// wildcard runs (`**`), `$` tables and `information_schema`.
    const S_CATALOGS: &[&str] = &["lake", "LAKE", "Lake", "hive", "*", "l*", "la?e", "**"];
    const S_SCHEMAS: &[&str] = &[
        "s1",
        "S1",
        "sales",
        "information_schema",
        "INFORMATION_SCHEMA",
        "*",
        "**",
        "s?",
        "s*",
    ];
    const S_TABLES: &[&str] = &["t", "T", "t$files", "orders", "*", "t*", "**", "?"];
    const S_COLUMNS: &[&str] = &["a", "A", "email", "*", "e*", "**"];

    /// Request values: the same literals in other cases, names nothing
    /// matches literally, `$` metadata tables, and `*` or empty.
    const R_CATALOGS: &[&str] = &["lake", "LaKe", "hive", "HIVE", "other", "*"];
    const R_SCHEMAS: &[&str] = &[
        "s1",
        "S1",
        "sales",
        "Sales",
        "information_schema",
        "x",
        "*",
        "",
    ];
    const R_TABLES: &[&str] = &[
        "t",
        "T",
        "t$files",
        "T$Snapshots",
        "orders$files",
        "orders",
        "x",
        "*",
        "",
    ];
    const R_COLUMNS: &[&str] = &["a", "A", "email", "Email", "b", "*"];

    const CLASSES: &[&str] = &["pii", "PII", "fin", "other"];

    /// splitmix64: deterministic, no dependency.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
        fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
            xs[self.below(xs.len())]
        }
        fn chance(&mut self, percent: u64) -> bool {
            self.next() % 100 < percent
        }
    }

    fn member(n: &str) -> Member {
        Member {
            name: if n == "*" {
                MemberName::Everyone
            } else {
                MemberName::User(n.into())
            },
            via: Via::Direct,
        }
    }

    fn audience(r: &mut Rng) -> Audience {
        let include = vec![member(r.pick(&["ann", "bob", LINEAGE, "*", "*"]))];
        let except = if r.chance(15) {
            vec![member(r.pick(&["ann", "bob"]))]
        } else {
            vec![]
        };
        Audience { include, except }
    }

    /// A path Snapshot::build accepts: no named level below an unnamed one.
    fn path(r: &mut Rng) -> Path {
        let mut unnamed = false;
        let mut level = |r: &mut Rng, pool: &[&str]| {
            if unnamed || r.chance(25) {
                unnamed = true;
                Level::Unnamed
            } else {
                Level::Named(r.pick(pool).into())
            }
        };
        let schema = level(r, S_SCHEMAS);
        let table = level(r, S_TABLES);
        let column = level(r, S_COLUMNS);
        Path {
            catalog_id: None,
            catalog: r.pick(S_CATALOGS).into(),
            schema,
            table,
            column,
        }
    }

    fn privileges(r: &mut Rng) -> Vec<Privilege> {
        match r.below(4) {
            0 => vec![Privilege::Read],
            1 => vec![Privilege::Write],
            2 => vec![Privilege::Read, Privilege::Write],
            _ => vec![],
        }
    }

    fn effect(r: &mut Rng) -> Effect {
        match r.below(11) {
            0..=2 => Effect::Allow {
                privileges: privileges(r),
            },
            3 => Effect::Deny {
                privileges: privileges(r),
            },
            4 => Effect::Withhold {
                privileges: privileges(r),
            },
            5 => Effect::Mask {
                mask_type: "MASK_HASH".into(),
                expression: None,
            },
            6 => Effect::Mask {
                mask_type: r.pick(&["MASK_NONE", "MASK_NULL"]).into(),
                expression: None,
            },
            7 => Effect::Mask {
                mask_type: "CUSTOM".into(),
                expression: Some("'x'".into()),
            },
            8 => Effect::WithholdMask,
            _ => Effect::RowFilter {
                expression: r.pick(&["a > 1", "b < 2", ""]).into(),
            },
        }
    }

    fn statement(r: &mut Rng, n: usize) -> Statement {
        if r.chance(3) {
            return Statement {
                source: Source::Infra {
                    kind: InfraKind::AdminAllAccess,
                },
                audience: Audience {
                    include: vec![member(r.pick(&["bob", LINEAGE]))],
                    except: vec![],
                },
                target: Target::Infra(InfraObject::WholeEp),
                effect: Effect::EnginePrivilege {
                    capability: "trino.all".into(),
                },
                derivation: Derivation::AsDeclared,
            };
        }
        let target = if r.chance(15) {
            Target::Classification {
                name: r.pick(CLASSES).into(),
            }
        } else {
            Target::Path(path(r))
        };
        Statement {
            source: Source::Rule {
                rule_id: uuid::Uuid::from_u128(n as u128),
                rule_name: format!("s{n}"),
            },
            audience: audience(r),
            target,
            effect: effect(r),
            derivation: Derivation::AsDeclared,
        }
    }

    fn classified(r: &mut Rng) -> ClassifiedColumn {
        ClassifiedColumn {
            catalog: r.pick(&["lake", "LAKE", "hive"]).into(),
            schema: r.pick(&["s1", "S1", "sales"]).into(),
            table: r.pick(&["t", "T", "orders"]).into(),
            // None: a table-level row (`column_name = ''` in the database).
            column: (!r.chance(30)).then(|| r.pick(&["a", "A", "email"]).into()),
            classification: r.pick(CLASSES).into(),
        }
    }

    fn snapshot(r: &mut Rng) -> Snapshot {
        let n = 1 + r.below(30);
        let statements = (0..n).map(|i| statement(r, i)).collect();
        let classified = (0..r.below(6)).map(|_| classified(r)).collect();
        Snapshot::build(SnapshotInput {
            service_id: 1,
            key: (1, 1),
            statements,
            classified,
            lineage_principal: LINEAGE.into(),
        })
        .expect("generated paths are well formed")
    }

    fn columns(r: &mut Rng) -> Vec<String> {
        (0..r.below(4)).map(|_| r.pick(R_COLUMNS).into()).collect()
    }

    fn resource(r: &mut Rng) -> Resource {
        let catalog = r.pick(R_CATALOGS).to_string();
        let schema = r.pick(R_SCHEMAS).to_string();
        let table = r.pick(R_TABLES).to_string();
        match r.below(6) {
            0 => Resource::Catalog(catalog),
            1 => Resource::Schema { catalog, schema },
            2 => Resource::Column {
                catalog,
                schema,
                table,
                column: r.pick(R_COLUMNS).into(),
                column_type: "varchar".into(),
            },
            _ => Resource::Table {
                catalog,
                schema,
                table,
                columns: columns(r),
            },
        }
    }

    fn object(r: &mut Rng) -> ObjectPath {
        let depth = r.below(4);
        let level =
            |r: &mut Rng, d: usize, pool: &[&str]| (depth >= d).then(|| r.pick(pool).into());
        ObjectPath {
            catalog: r.pick(R_CATALOGS).into(),
            schema: level(r, 1, R_SCHEMAS),
            table: level(r, 2, R_TABLES),
            column: level(r, 3, R_COLUMNS),
        }
    }

    /// One question and its answer, rendered so that index and oracle
    /// answers compare exactly.
    #[derive(Debug, PartialEq)]
    enum Answer {
        Decide(Decision),
        Filter(Vec<usize>),
        RowFilters(Result<Vec<String>, crate::decide::EvalError>),
        Mask(Result<Option<(String, Option<String>)>, crate::decide::EvalError>),
        Masks(Result<std::collections::BTreeMap<String, String>, crate::decide::EvalError>),
    }

    /// The operations whose answers depend on the path statements.
    fn data_ops() -> Vec<&'static str> {
        TRINO_483_OPERATIONS
            .iter()
            .filter(|o| matches!(o.class, OpClass::Data(_) | OpClass::Reach))
            .map(|o| o.name)
            .collect()
    }

    /// The questions asked of one snapshot, answered by `s` as it stands.
    fn ask(s: &Snapshot, questions: &[Question]) -> Vec<Answer> {
        questions
            .iter()
            .map(|q| match q {
                Question::Decide(u, op, res) => Answer::Decide(s.decide(u, op, res)),
                Question::Filter(u, op, res) => Answer::Filter(s.filter(u, op, res)),
                Question::Privilege(u, p, o) => Answer::Decide(s.decide_privilege(u, *p, o)),
                Question::RowFilters(u, c, sc, t) => Answer::RowFilters(s.row_filters(u, c, sc, t)),
                Question::Mask(u, c, sc, t, col) => {
                    Answer::Mask(s.column_mask_choice(u, c, sc, t, col))
                }
                Question::Masks(u, c, sc, t, cols) => {
                    Answer::Masks(s.column_masks(u, c, sc, t, cols))
                }
            })
            .collect()
    }

    #[derive(Debug)]
    enum Question {
        Decide(&'static str, &'static str, Resource),
        Filter(&'static str, &'static str, Vec<Resource>),
        Privilege(&'static str, DataPrivilege, ObjectPath),
        RowFilters(&'static str, String, String, String),
        Mask(&'static str, String, String, String, String),
        Masks(&'static str, String, String, String, Vec<(String, String)>),
    }

    fn questions(r: &mut Rng, ops: &[&'static str]) -> Vec<Question> {
        let mut out = Vec::new();
        for _ in 0..40 {
            let user = USERS[r.below(USERS.len())];
            let (c, sc, t) = (
                r.pick(R_CATALOGS).to_string(),
                r.pick(R_SCHEMAS).to_string(),
                r.pick(R_TABLES).to_string(),
            );
            out.push(match r.below(7) {
                0 | 1 => Question::Decide(user, ops[r.below(ops.len())], resource(r)),
                2 => Question::Filter(
                    user,
                    r.pick(&["FilterTables", "FilterColumns", "FilterSchemas"]),
                    (0..1 + r.below(4)).map(|_| resource(r)).collect(),
                ),
                3 => Question::Privilege(
                    user,
                    if r.chance(70) {
                        DataPrivilege::Read
                    } else {
                        DataPrivilege::Write
                    },
                    object(r),
                ),
                4 => Question::RowFilters(user, c, sc, t),
                5 => Question::Mask(user, c, sc, t, r.pick(R_COLUMNS).into()),
                _ => Question::Masks(
                    user,
                    c,
                    sc,
                    t,
                    columns(r)
                        .into_iter()
                        .map(|c| (c, "varchar".to_string()))
                        .collect(),
                ),
            });
        }
        out
    }

    /// Task 22F: on generated snapshots and questions, the indexed lookup
    /// answers exactly as the linear scan does. The tallies make sure the
    /// generator reaches the answers that matter: allows, explicit refusals
    /// (deny wins), masks, row filters and two-mask errors.
    #[test]
    fn the_index_answers_as_the_linear_scan_does() {
        let ops = data_ops();
        let mut r = Rng(0x22F0_0001);
        let (mut allows, mut explicit, mut masks, mut filters, mut two_masks) = (0, 0, 0, 0, 0);
        for round in 0..3000 {
            let s = snapshot(&mut r);
            let qs = questions(&mut r, &ops);
            let indexed = ask(&s, &qs);
            let linear = with_linear(|| ask(&s, &qs));
            for ((q, a), b) in qs.iter().zip(&indexed).zip(&linear) {
                assert_eq!(
                    a, b,
                    "round {round}: the index and the linear scan disagree on {q:?}\n\
                     statements: {:#?}",
                    s.statements
                );
                match a {
                    Answer::Decide(Decision::Allow { .. }) => allows += 1,
                    Answer::Decide(Decision::Deny { explicit: true, .. }) => explicit += 1,
                    Answer::Filter(v) if !v.is_empty() => allows += 1,
                    Answer::Mask(Ok(Some(_))) => masks += 1,
                    Answer::Masks(Ok(m)) if !m.is_empty() => masks += 1,
                    Answer::RowFilters(Ok(v)) if !v.is_empty() => filters += 1,
                    Answer::Mask(Err(_)) | Answer::Masks(Err(_)) => two_masks += 1,
                    _ => {}
                }
            }
        }
        eprintln!(
            "allows {allows}, explicit refusals {explicit}, masks {masks}, \
             row filters {filters}, two-mask errors {two_masks}"
        );
        for (what, n) in [
            ("allows", allows),
            ("explicit refusals", explicit),
            ("masks", masks),
            ("row filters", filters),
            ("two-mask errors", two_masks),
        ] {
            assert!(n >= 200, "the generator reached only {n} {what}");
        }
    }

    /// The oracle is a different code path: on a snapshot whose index would
    /// leave a statement out, it still returns that statement. Guards the
    /// oracle itself from silently using the index.
    #[test]
    fn the_oracle_scans_every_statement() {
        let mut r = Rng(7);
        let s = snapshot(&mut r);
        let all =
            with_linear(|| s.candidates("nothing", Some("x"), Some("y"), &Default::default()));
        assert_eq!(all, (0..s.len()).collect::<Vec<_>>());
    }
}
