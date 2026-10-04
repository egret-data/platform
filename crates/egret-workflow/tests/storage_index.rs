//! W9a Task 3: the file index (`workflow.execution_files`, migration 0161),
//! the readable set and `storage::resolve_readable_many` -- the ONE function
//! that decides whether a URI is readable (spec §4.4). Requires DATABASE_URL.
//!
//! Every test makes its own workspaces and executions, so the tests cannot see
//! each other's rows and run in parallel. Executions are bare rows inserted
//! directly: the readable set is a property of the link columns
//! (`parent_execution_id` + `loop_index`, `trigger_execution_id`,
//! `replayed_from_execution_id`), and a fixture that drove real flows to get
//! them would test the engine instead.
//!
//! No test here touches an object store: the index is Postgres only.

mod common;

use egret_metastore::Db;
use egret_workflow::storage::{
    self, index_input_files, index_task_files, loop_family, loop_root, readable_executions,
    resolve_readable, resolve_readable_many, uri, IndexedFile, StoredFile, Unreadable,
};
use uuid::Uuid;

const NS: &str = "w9a.t3";
const FLOW: &str = "Files_Flow";

/// How an execution row is linked.
#[derive(Default, Clone, Copy)]
struct Link {
    /// A loop iteration of this execution (`loop_index` = `index`).
    iteration_of: Option<(Uuid, i32)>,
    /// A retry replay of this execution (`parent_execution_id`, no loop index).
    retry_of: Option<Uuid>,
    /// A manual replay of this execution.
    replay_of: Option<Uuid>,
    /// Fired by / called from this execution.
    trigger: Option<Uuid>,
}

async fn exec(db: &Db, ws: Uuid, link: Link) -> Uuid {
    let (parent, run_ix, loop_index, loop_value) = match (link.iteration_of, link.retry_of) {
        (Some((p, i)), None) => (Some(p), Some(0), Some(i), Some(format!("v{i}"))),
        (None, Some(p)) => (Some(p), None, None, None),
        (None, None) => (None, None, None, None),
        _ => panic!("an iteration or a retry replay, not both"),
    };
    sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (workspace_id, namespace, flow_key, source, state, history, parent_execution_id, \
            parent_run_ix, loop_index, loop_value, trigger_execution_id, \
            replayed_from_execution_id) \
         VALUES ($1, $2, $3, 'id: f\ntasks: []\n', 'SUCCESS', '[\"CREATED\",\"SUCCESS\"]'::jsonb, \
                 $4, $5, $6, $7, $8, $9) RETURNING id",
    )
    .bind(ws)
    .bind(NS)
    .bind(FLOW)
    .bind(parent)
    .bind(run_ix)
    .bind(loop_index)
    .bind(loop_value)
    .bind(link.trigger)
    .bind(link.replay_of)
    .fetch_one(&db.pool)
    .await
    .expect("insert execution")
}

async fn root(db: &Db, ws: Uuid) -> Uuid {
    exec(db, ws, Link::default()).await
}

async fn task_run(db: &Db, execution: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workflow.task_runs \
           (execution_id, run_ix, task_ix, task_id, type_id, state, history) \
         VALUES ($1, (SELECT count(*) FROM workflow.task_runs WHERE execution_id = $1), 0, \
                 'Write_It', 'io.kestra.plugin.core.storage.Write', 'SUCCESS', \
                 '[\"SUCCESS\"]'::jsonb) RETURNING id",
    )
    .bind(execution)
    .fetch_one(&db.pool)
    .await
    .expect("insert task run")
}

/// A file a task run of `execution` wrote, indexed through the real writer.
async fn write_file(db: &Db, ws: Uuid, execution: Uuid, name: &str, size: u64) -> String {
    let run = task_run(db, execution).await;
    let uri = format!(
        "{}{name}",
        uri::task_prefix(NS, FLOW, execution, "Write_It", run).unwrap()
    );
    let mut conn = db.pool.acquire().await.unwrap();
    index_task_files(
        &mut conn,
        ws,
        run,
        &[StoredFile {
            uri: uri.clone(),
            size,
        }],
    )
    .await
    .expect("index the file");
    uri
}

fn sorted(mut v: Vec<Uuid>) -> Vec<Uuid> {
    v.sort();
    v
}

#[tokio::test]
async fn a_loop_iteration_resolves_to_its_root_and_a_retry_replay_does_not() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let r = root(&db, ws).await;
    let i = exec(
        &db,
        ws,
        Link {
            iteration_of: Some((r, 0)),
            ..Link::default()
        },
    )
    .await;
    let nested = exec(
        &db,
        ws,
        Link {
            iteration_of: Some((i, 0)),
            ..Link::default()
        },
    )
    .await;
    let retry = exec(
        &db,
        ws,
        Link {
            retry_of: Some(r),
            ..Link::default()
        },
    )
    .await;
    let mut conn = db.pool.acquire().await.unwrap();

    assert_eq!(
        loop_root(&mut conn, ws, r).await.unwrap(),
        r,
        "a root is its own"
    );
    assert_eq!(loop_root(&mut conn, ws, i).await.unwrap(), r);
    assert_eq!(
        loop_root(&mut conn, ws, nested).await.unwrap(),
        r,
        "at any depth"
    );
    // A retry replay's parent is its SOURCE, not its loop: the walk stops.
    assert_eq!(loop_root(&mut conn, ws, retry).await.unwrap(), retry);

    assert_eq!(
        sorted(loop_family(&mut conn, ws, nested).await.unwrap()),
        sorted(vec![r, i, nested]),
        "the family is the root and every iteration under it -- not the retry replay"
    );
    assert_eq!(
        loop_family(&mut conn, ws, retry).await.unwrap(),
        vec![retry]
    );

    // Scoped by workspace: another workspace's id is not found.
    let (other_ws, _) = common::seed_workspace(&db).await;
    assert!(loop_root(&mut conn, other_ws, i).await.is_err());
}

/// The chain of spec §4.4: root R (trigger T), retry-replay R1 of R, manual
/// replay R2 of R1 (trigger T2); loop iterations I and I2 of R2. J is an
/// iteration of the chain link R1 and is EXCLUDED (spec §4.4: the replay
/// chain's own loop iterations are not in the set); U is unrelated. T was
/// itself triggered by T0, and T0 is EXCLUDED: the trigger rule is one level
/// (fix round 1 M2).
/// W9a Task 13 fix round 2: `readable_from_any` (the pruner's set form, the
/// same SQL seeded with `= ANY($2)`) is the union of `readable_executions`
/// over its seeds -- across loop, trigger, retry and manual-replay links.
#[tokio::test]
async fn readable_from_any_is_the_union_of_the_single_readable_sets() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let r = root(&db, ws).await;
    let i = exec(
        &db,
        ws,
        Link {
            iteration_of: Some((r, 0)),
            ..Link::default()
        },
    )
    .await;
    let child = exec(
        &db,
        ws,
        Link {
            trigger: Some(i),
            ..Link::default()
        },
    )
    .await;
    let retry = exec(
        &db,
        ws,
        Link {
            retry_of: Some(r),
            ..Link::default()
        },
    )
    .await;
    let replay = exec(
        &db,
        ws,
        Link {
            replay_of: Some(retry),
            ..Link::default()
        },
    )
    .await;
    let lone = root(&db, ws).await;
    let mut conn = db.pool.acquire().await.unwrap();
    for seeds in [
        vec![child],
        vec![child, replay],
        vec![i, lone],
        vec![r, i, child, retry, replay, lone],
        vec![],
    ] {
        let mut union = std::collections::BTreeSet::new();
        for s in &seeds {
            union.extend(readable_executions(&mut conn, ws, *s).await.unwrap());
        }
        let set: std::collections::BTreeSet<Uuid> =
            storage::readable_from_any(&mut conn, ws, &seeds)
                .await
                .unwrap()
                .into_iter()
                .collect();
        assert_eq!(set, union, "{seeds:?}");
    }
}

#[tokio::test]
async fn readable_covers_own_root_trigger_retry_replay_and_manual_replay_chain() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let t0 = root(&db, ws).await;
    let t = exec(
        &db,
        ws,
        Link {
            trigger: Some(t0),
            ..Link::default()
        },
    )
    .await;
    let t2 = root(&db, ws).await;
    let r = exec(
        &db,
        ws,
        Link {
            trigger: Some(t),
            ..Link::default()
        },
    )
    .await;
    let r1 = exec(
        &db,
        ws,
        Link {
            retry_of: Some(r),
            ..Link::default()
        },
    )
    .await;
    let r2 = exec(
        &db,
        ws,
        Link {
            replay_of: Some(r1),
            trigger: Some(t2),
            ..Link::default()
        },
    )
    .await;
    let i = exec(
        &db,
        ws,
        Link {
            iteration_of: Some((r2, 0)),
            ..Link::default()
        },
    )
    .await;
    let i2 = exec(
        &db,
        ws,
        Link {
            iteration_of: Some((r2, 1)),
            ..Link::default()
        },
    )
    .await;
    let j = exec(
        &db,
        ws,
        Link {
            iteration_of: Some((r1, 0)),
            ..Link::default()
        },
    )
    .await;
    let u = root(&db, ws).await;
    let mut conn = db.pool.acquire().await.unwrap();

    let got = sorted(readable_executions(&mut conn, ws, i).await.unwrap());
    assert_eq!(got, sorted(vec![i, i2, r2, r1, r, t, t2]));
    assert!(
        !got.contains(&j),
        "a replay chain link's own iteration is not readable"
    );
    assert!(!got.contains(&u));
    assert!(
        !got.contains(&t0),
        "a trigger's own trigger is not readable: one level"
    );
    // From the root, the same set: the root's family holds both iterations.
    assert_eq!(
        sorted(readable_executions(&mut conn, ws, r2).await.unwrap()),
        got
    );
    // The chain runs one way: R reads neither its replays nor their files.
    assert_eq!(
        sorted(readable_executions(&mut conn, ws, r).await.unwrap()),
        sorted(vec![r, t])
    );
    // Another workspace's caller gets nothing of this one.
    let (other_ws, _) = common::seed_workspace(&db).await;
    assert!(readable_executions(&mut conn, other_ws, i)
        .await
        .unwrap()
        .is_empty());
}

/// **The user's ruling of 2026-10-01 on Task 7's concern 1**: the readable
/// set also holds the LOOP FAMILY of each trigger execution -- its loop root
/// and every iteration under that root -- still one trigger level. A child
/// started from inside iteration `it0` (a `Subflow` there names `it0` as its
/// trigger) reads the loop root's files and a sibling iteration's; the
/// trigger's own trigger `t0` and an unrelated execution stay unreadable.
#[tokio::test]
async fn a_triggers_loop_family_is_readable_and_the_triggers_trigger_is_not() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let t0 = root(&db, ws).await;
    let tr = exec(
        &db,
        ws,
        Link {
            trigger: Some(t0),
            ..Link::default()
        },
    )
    .await;
    let it0 = exec(
        &db,
        ws,
        Link {
            iteration_of: Some((tr, 0)),
            ..Link::default()
        },
    )
    .await;
    let it1 = exec(
        &db,
        ws,
        Link {
            iteration_of: Some((tr, 1)),
            ..Link::default()
        },
    )
    .await;
    // A nested loop under the sibling: still the trigger's loop family.
    let it1a = exec(
        &db,
        ws,
        Link {
            iteration_of: Some((it1, 0)),
            ..Link::default()
        },
    )
    .await;
    let child = exec(
        &db,
        ws,
        Link {
            trigger: Some(it0),
            ..Link::default()
        },
    )
    .await;
    let u = root(&db, ws).await;
    let mut conn = db.pool.acquire().await.unwrap();

    let got = sorted(readable_executions(&mut conn, ws, child).await.unwrap());
    assert_eq!(got, sorted(vec![child, it0, it1, it1a, tr]));
    assert!(
        !got.contains(&t0),
        "the trigger's own trigger: one level only"
    );
    assert!(!got.contains(&u));

    let sibling = write_file(&db, ws, it1, "sibling.txt", 3).await;
    let of_root = write_file(&db, ws, tr, "root.txt", 4).await;
    let of_t0 = write_file(&db, ws, t0, "t0.txt", 5).await;
    for (uri, size) in [(&sibling, 3), (&of_root, 4)] {
        let got = resolve_readable(&mut conn, ws, child, uri).await.unwrap();
        assert_eq!(got.map(|f| f.size), Ok(size), "{uri}");
    }
    assert_eq!(
        resolve_readable(&mut conn, ws, child, &of_t0)
            .await
            .unwrap(),
        Err(Unreadable::NotReadable { uri: of_t0.clone() })
    );
    // A trigger that is NOT an iteration contributes itself only (its family
    // is its own iterations, of which `t0` has none).
    assert_eq!(
        sorted(readable_executions(&mut conn, ws, tr).await.unwrap()),
        sorted(vec![tr, it0, it1, it1a, t0])
    );
}

/// The user's ruling (spec §3): a file written in iteration I carries I's id.
/// It is readable from the root (a task after the loop) and from a sibling.
#[tokio::test]
async fn a_task_after_the_loop_and_a_sibling_read_an_iterations_file() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let r2 = root(&db, ws).await;
    let i = exec(
        &db,
        ws,
        Link {
            iteration_of: Some((r2, 0)),
            ..Link::default()
        },
    )
    .await;
    let i2 = exec(
        &db,
        ws,
        Link {
            iteration_of: Some((r2, 1)),
            ..Link::default()
        },
    )
    .await;
    let uri = write_file(&db, ws, i, "it.txt", 7).await;
    assert_eq!(
        uri::parse(&uri).unwrap().execution,
        i,
        "the URI's {{exec}} is I"
    );
    let mut conn = db.pool.acquire().await.unwrap();

    for reader in [r2, i2, i] {
        let got = resolve_readable(&mut conn, ws, reader, &uri).await.unwrap();
        let file = got.unwrap_or_else(|e| panic!("from {reader}: {e}"));
        assert_eq!((file.execution_id, file.size), (i, 7));
    }
}

#[tokio::test]
async fn a_file_written_in_an_iteration_is_indexed_under_the_iteration() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let r2 = root(&db, ws).await;
    let i = exec(
        &db,
        ws,
        Link {
            iteration_of: Some((r2, 0)),
            ..Link::default()
        },
    )
    .await;
    let uri = write_file(&db, ws, i, "out.csv", 3).await;
    let (execution_id, task_run_id): (Uuid, Option<Uuid>) = sqlx::query_as(
        "SELECT execution_id, task_run_id FROM workflow.execution_files \
          WHERE workspace_id = $1 AND uri = $2",
    )
    .bind(ws)
    .bind(&uri)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(execution_id, i, "the iteration, not the root {r2}");
    assert!(task_run_id.is_some());

    // A URI naming the ROOT for the iteration's task run is egret's own bug.
    let run = task_run(&db, i).await;
    let wrong = format!(
        "{}x",
        uri::task_prefix(NS, FLOW, r2, "Write_It", run).unwrap()
    );
    let mut conn = db.pool.acquire().await.unwrap();
    let err = index_task_files(
        &mut conn,
        ws,
        run,
        &[StoredFile {
            uri: wrong.clone(),
            size: 1,
        }],
    )
    .await
    .unwrap_err();
    assert!(matches!(err, egret_core::Error::Internal(_)), "{err}");
    assert!(err.to_string().contains(&wrong), "{err}");
    // So is a URI naming another task run of the right execution.
    let other_run = format!(
        "{}x",
        uri::task_prefix(NS, FLOW, i, "Write_It", Uuid::new_v4()).unwrap()
    );
    assert!(index_task_files(
        &mut conn,
        ws,
        run,
        &[StoredFile {
            uri: other_run,
            size: 1
        }]
    )
    .await
    .is_err());

    // A retried task run writes the same URI: the row is updated, not doubled.
    let again = write_file_for_run(&db, ws, i, run, "y", 5).await;
    let again2 = write_file_for_run(&db, ws, i, run, "y", 9).await;
    assert_eq!(again, again2);
    let sizes: Vec<i64> = sqlx::query_scalar(
        "SELECT size FROM workflow.execution_files WHERE workspace_id = $1 AND uri = $2",
    )
    .bind(ws)
    .bind(&again)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(sizes, vec![9]);

    // Input files: no task run, and only `inputs/` URIs of the execution.
    let input = uri::input_uri(NS, FLOW, r2, "file1", "data.csv").unwrap();
    index_input_files(
        &mut conn,
        ws,
        r2,
        &[StoredFile {
            uri: input.clone(),
            size: 4,
        }],
    )
    .await
    .unwrap();
    let got = resolve_readable(&mut conn, ws, i, &input)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        got,
        IndexedFile {
            uri: input,
            execution_id: r2,
            task_run_id: None,
            size: 4
        }
    );
    let not_input = format!(
        "{}z",
        uri::task_prefix(NS, FLOW, r2, "t", Uuid::new_v4()).unwrap()
    );
    assert!(index_input_files(
        &mut conn,
        ws,
        r2,
        &[StoredFile {
            uri: not_input,
            size: 1
        }]
    )
    .await
    .is_err());
    let other_exec = uri::input_uri(NS, FLOW, i, "file1", "data.csv").unwrap();
    assert!(index_input_files(
        &mut conn,
        ws,
        r2,
        &[StoredFile {
            uri: other_exec,
            size: 1
        }]
    )
    .await
    .is_err());
}

async fn write_file_for_run(
    db: &Db,
    ws: Uuid,
    execution: Uuid,
    run: Uuid,
    name: &str,
    size: u64,
) -> String {
    let uri = format!(
        "{}{name}",
        uri::task_prefix(NS, FLOW, execution, "Write_It", run).unwrap()
    );
    let mut conn = db.pool.acquire().await.unwrap();
    index_task_files(
        &mut conn,
        ws,
        run,
        &[StoredFile {
            uri: uri.clone(),
            size,
        }],
    )
    .await
    .unwrap();
    uri
}

/// THE probe: the other execution's row EXISTS. Removing the `readable`
/// filter makes it `Ok`. And the answer is the same for a URI of that
/// execution that names no file -- it never says whether the file exists.
#[tokio::test]
async fn a_file_of_an_unrelated_execution_in_the_same_workspace_is_not_readable() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let mine = root(&db, ws).await;
    let theirs = root(&db, ws).await;
    let present = write_file(&db, ws, theirs, "secret.txt", 6).await;
    let absent = present.replace("secret.txt", "nope.txt");
    let mut conn = db.pool.acquire().await.unwrap();

    // The control: the owner reads it.
    assert!(resolve_readable(&mut conn, ws, theirs, &present)
        .await
        .unwrap()
        .is_ok());
    for uri in [&present, &absent] {
        let got = resolve_readable(&mut conn, ws, mine, uri).await.unwrap();
        assert_eq!(got, Err(Unreadable::NotReadable { uri: uri.clone() }));
    }
}

/// Identical URI strings in two workspaces (a URI carries no workspace):
/// ws A's row has size 1, ws B's size 2. A reads 1; nobody else ever sees 2
/// through A's URI, even a caller that passes A's execution in its readable
/// set from workspace C (the SELECT is scoped by workspace).
#[tokio::test]
async fn the_same_uri_in_two_workspaces_resolves_to_the_callers_row_only() {
    let Some(db) = common::db().await else { return };
    let (a, _) = common::seed_workspace(&db).await;
    let (b, _) = common::seed_workspace(&db).await;
    let (c, _) = common::seed_workspace(&db).await;
    let ea = root(&db, a).await;
    let eb = root(&db, b).await;
    let ec = root(&db, c).await;
    let uri = uri::input_uri(NS, FLOW, ea, "f", "x.txt").unwrap();
    for (ws, e, size) in [(a, ea, 1_i64), (b, eb, 2)] {
        // Raw: B's row names B's execution with A's URI, which no writer of
        // egret's would produce -- that is the point of the probe.
        sqlx::query(
            "INSERT INTO workflow.execution_files (workspace_id, execution_id, uri, size) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(ws)
        .bind(e)
        .bind(&uri)
        .bind(size)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    let mut conn = db.pool.acquire().await.unwrap();
    let from_a = resolve_readable(&mut conn, a, ea, &uri)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(from_a.size, 1);
    assert_eq!(
        resolve_readable(&mut conn, c, ec, &uri).await.unwrap(),
        Err(Unreadable::NotReadable { uri: uri.clone() })
    );
    let forced = resolve_readable_many(&mut conn, c, &[ea], std::slice::from_ref(&uri))
        .await
        .unwrap();
    assert_eq!(forced[&uri], Err(Unreadable::Missing { uri: uri.clone() }));
}

#[tokio::test]
async fn a_readable_execution_with_no_such_row_is_missing_not_unreadable() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let e = root(&db, ws).await;
    let present = write_file(&db, ws, e, "here.txt", 0).await;
    let missing = present.replace("here.txt", "nope.txt");
    let malformed = "kestra:///a/../b/executions/x/y".to_string();
    let not_kestra = "s3://warehouse/x".to_string();
    let mut conn = db.pool.acquire().await.unwrap();
    let readable = readable_executions(&mut conn, ws, e).await.unwrap();
    let got = resolve_readable_many(
        &mut conn,
        ws,
        &readable,
        &[
            present.clone(),
            missing.clone(),
            malformed.clone(),
            not_kestra.clone(),
        ],
    )
    .await
    .unwrap();
    assert_eq!(got.len(), 4);
    assert_eq!(got[&present].as_ref().map(|f| f.size), Ok(0));
    assert_eq!(
        got[&missing],
        Err(Unreadable::Missing {
            uri: missing.clone()
        })
    );
    for m in [&malformed, &not_kestra] {
        match &got[m] {
            Err(Unreadable::Malformed { uri, reason }) => {
                assert_eq!(uri, m);
                assert!(!reason.is_empty());
            }
            other => panic!("{m}: {other:?}"),
        }
    }
    assert!(got[&missing]
        .clone()
        .unwrap_err()
        .to_string()
        .contains(&missing));
}

#[tokio::test]
async fn the_manual_replay_route_records_its_source() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage"]).await;
    let source = format!(
        "id: replay-link\nnamespace: {NS}\ntasks:\n  \
         - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    );
    let flow = common::seed_flow(&db, ws, user, &source, "draft").await;
    let done: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (workspace_id, flow_id, namespace, flow_key, source, state, history) \
         VALUES ($1, $2, $3, 'replay-link', $4, 'SUCCESS', '[\"CREATED\",\"SUCCESS\"]'::jsonb) \
         RETURNING id",
    )
    .bind(ws)
    .bind(flow)
    .bind(NS)
    .bind(&source)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let app = common::app(&db).await;
    let res = common::post_no_body(
        &app,
        &format!("/api/ws/{ws}/workflows/executions/{done}/replay"),
        user,
    )
    .await;
    assert_eq!(res.status(), 201);
    let replacement: Uuid = common::json(res).await["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let (link, parent): (Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT replayed_from_execution_id, parent_execution_id \
           FROM workflow.executions WHERE id = $1",
    )
    .bind(replacement)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!((link, parent), (Some(done), None));
    // And so the replay reads its source's files.
    let mut conn = db.pool.acquire().await.unwrap();
    assert!(readable_executions(&mut conn, ws, replacement)
        .await
        .unwrap()
        .contains(&done));
}

/// User ruling (fix round 1): the manual replay route refuses a loop
/// iteration, naming its loop root, as `replay::spawn_due` never replays one;
/// the root itself still replays (the control). No execution is created by
/// the refusal.
#[tokio::test]
async fn the_manual_replay_route_refuses_a_loop_iteration_and_replays_its_root() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage"]).await;
    let source = format!(
        "id: replay-iter\nnamespace: {NS}\ntasks:\n  \
         - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    );
    let flow = common::seed_flow(&db, ws, user, &source, "draft").await;
    let insert = |parent: Option<Uuid>| {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO workflow.executions \
               (workspace_id, flow_id, namespace, flow_key, source, state, history, \
                parent_execution_id, parent_run_ix, loop_index, loop_value) \
             VALUES ($1, $2, $3, 'replay-iter', $4, 'SUCCESS', '[\"CREATED\",\"SUCCESS\"]'::jsonb, \
                     $5, $6, $7, $8) RETURNING id",
        )
        .bind(ws)
        .bind(flow)
        .bind(NS)
        .bind(source.clone())
        .bind(parent)
        .bind(parent.map(|_| 0))
        .bind(parent.map(|_| 2))
        .bind(parent.map(|_| "v".to_string()))
        .fetch_one(&db.pool)
    };
    let root = insert(None).await.unwrap();
    let iteration = insert(Some(root)).await.unwrap();
    let app = common::app(&db).await;
    let before = common::execution_count(&db, ws).await;

    let res = common::post_no_body(
        &app,
        &format!("/api/ws/{ws}/workflows/executions/{iteration}/replay"),
        user,
    )
    .await;
    assert_eq!(res.status(), 400);
    let body = common::json(res).await.to_string();
    assert!(body.contains("iteration 2 of a Loop"), "{body}");
    assert!(
        body.contains(&format!("replay its loop root {root}")),
        "{body}"
    );
    assert_eq!(
        common::execution_count(&db, ws).await,
        before,
        "nothing started"
    );

    let res = common::post_no_body(
        &app,
        &format!("/api/ws/{ws}/workflows/executions/{root}/replay"),
        user,
    )
    .await;
    assert_eq!(res.status(), 201, "the loop root replays");
}

#[tokio::test]
async fn the_composite_fk_refuses_a_file_row_naming_another_workspaces_execution() {
    let Some(db) = common::db().await else { return };
    let (a, _) = common::seed_workspace(&db).await;
    let (b, _) = common::seed_workspace(&db).await;
    let eb = root(&db, b).await;
    let uri = uri::input_uri(NS, FLOW, eb, "f", "x").unwrap();
    let insert = |ws: Uuid| {
        sqlx::query(
            "INSERT INTO workflow.execution_files (workspace_id, execution_id, uri, size) \
             VALUES ($1, $2, $3, 1)",
        )
        .bind(ws)
        .bind(eb)
        .bind(uri.clone())
        .execute(&db.pool)
    };
    let err = insert(a).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("workflow_execution_files_execution_same_workspace"),
        "{err}"
    );
    insert(b).await.expect("the control: its own workspace");

    // The URI CHECK and the size CHECK, each with the row otherwise valid.
    for (u, size) in [("s3://b/k", 1_i64), ("kestra://x", 1), (uri.as_str(), -1)] {
        let r = sqlx::query(
            "INSERT INTO workflow.execution_files (workspace_id, execution_id, uri, size) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(b)
        .bind(eb)
        .bind(format!("{u}{}", if size < 0 { "-neg" } else { "" }))
        .bind(size)
        .execute(&db.pool)
        .await;
        assert!(r.is_err(), "{u} size {size} must be refused");
    }

    // The replay link: same workspace, never itself, never beside a parent.
    let ea = root(&db, a).await;
    let link = |e: Uuid, to: Uuid| {
        sqlx::query("UPDATE workflow.executions SET replayed_from_execution_id = $2 WHERE id = $1")
            .bind(e)
            .bind(to)
            .execute(&db.pool)
    };
    let err = link(ea, eb).await.unwrap_err().to_string();
    assert!(
        err.contains("workflow_executions_replayed_from_same_workspace"),
        "{err}"
    );
    let err = link(eb, eb).await.unwrap_err().to_string();
    assert!(
        err.contains("workflow_executions_replayed_from_not_self"),
        "{err}"
    );
    let child = exec(
        &db,
        b,
        Link {
            retry_of: Some(eb),
            ..Link::default()
        },
    )
    .await;
    let other = root(&db, b).await;
    let err = link(child, other).await.unwrap_err().to_string();
    assert!(
        err.contains("workflow_executions_replayed_from_is_not_a_child"),
        "{err}"
    );
    link(other, eb)
        .await
        .expect("the control: a top-level replay of its own workspace");

    // Fix round 1 I2: the replay link has its partial index.
    let def: String = sqlx::query_scalar(
        "SELECT indexdef FROM pg_indexes \
          WHERE schemaname = 'workflow' AND indexname = 'workflow_executions_replayed_from_idx'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("the index exists");
    assert!(
        def.contains("(replayed_from_execution_id)")
            && def.contains("WHERE (replayed_from_execution_id IS NOT NULL)"),
        "{def}"
    );
}

#[tokio::test]
async fn deleting_a_workspace_cascades_its_file_rows() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let r = root(&db, ws).await;
    let replay = exec(
        &db,
        ws,
        Link {
            replay_of: Some(r),
            ..Link::default()
        },
    )
    .await;
    write_file(&db, ws, r, "a", 1).await;
    write_file(&db, ws, replay, "b", 1).await;
    let count = || {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM workflow.execution_files WHERE workspace_id = $1",
        )
        .bind(ws)
        .fetch_one(&db.pool)
    };
    assert_eq!(count().await.unwrap(), 2, "the control");
    sqlx::query("DELETE FROM workspaces WHERE id = $1")
        .bind(ws)
        .execute(&db.pool)
        .await
        .expect("delete the workspace, its replay link included");
    assert_eq!(count().await.unwrap(), 0);
}

/// `install_mem_for_tests` is idempotent and `current` returns what it
/// installed; a second `install` is refused and leaves the first.
#[tokio::test]
async fn the_process_wide_store_is_installed_once() {
    let first = storage::install_mem_for_tests();
    let again = storage::install_mem_for_tests();
    assert!(std::sync::Arc::ptr_eq(&first, &again));
    assert!(std::sync::Arc::ptr_eq(&storage::current().unwrap(), &first));
    let other = std::sync::Arc::new(storage::FileStore {
        store: std::sync::Arc::new(egret_storage::MemStore::new()),
        bucket: "other".into(),
        prefix: "p/".into(),
        caps: storage::Caps::default(),
        retention_days: 1,
    });
    assert!(storage::install(other).is_err());
    assert_eq!(storage::current().unwrap().bucket, first.bucket);
}

/// **Fix round 1, M5: a commit with no files touches no table.** Every
/// outcome commit calls `index_task_files`, almost always with nothing;
/// that must cost no query on the commit's transaction. Asked with a task
/// run that exists nowhere -- with a query, the workspace check would refuse
/// it -- and on a transaction already aborted, where ANY statement errors.
#[tokio::test]
async fn an_empty_file_list_is_a_no_op_that_queries_nothing() {
    let Some(db) = common::db().await else { return };
    let (ws, _) = common::seed_workspace(&db).await;
    let mut tx = db.pool.begin().await.unwrap();
    index_task_files(&mut tx, ws, Uuid::new_v4(), &[])
        .await
        .expect("nothing to index is not an error");
    let _ = sqlx::query("SELECT 1/0").execute(&mut *tx).await;
    index_task_files(&mut tx, ws, Uuid::new_v4(), &[])
        .await
        .expect("no statement was sent on the aborted transaction");
    // The control: with a file, the same run is refused.
    let err = index_task_files(
        &mut tx,
        ws,
        Uuid::new_v4(),
        &[StoredFile {
            uri: "kestra:///x/y/executions/00000000-0000-0000-0000-000000000001/a".into(),
            size: 1,
        }],
    )
    .await
    .unwrap_err();
    assert!(!err.to_string().is_empty());
}
