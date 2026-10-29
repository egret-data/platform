//! The readable set and the one readability decision (W9a spec §4.4).
//!
//! Every query is ONE recursive CTE, scoped by `workspace_id` in every step,
//! and runs on the caller's connection -- inside `settle` that is the
//! transaction that has just indexed the finished task's files, which a pool
//! would not see. `UNION` (not `UNION ALL`) in every recursive term, so a
//! cycle in the link columns ends the walk instead of looping.

use std::collections::{BTreeMap, BTreeSet};

use egret_core::{Error, Result};
use sqlx::PgConnection;
use uuid::Uuid;

use super::{uri, IndexedFile, Unreadable};

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::Error::new(e))
}

/// The walk of spec §3, shared by the three queries below (a macro so each
/// query stays one `&'static str`): `up` climbs
/// `parent_execution_id` from `$2` **while the row is a loop iteration**
/// (`loop_index IS NOT NULL`), and the root is the row it stops at -- the one
/// with no `loop_index`. A retry replay has `parent_execution_id` set and
/// `loop_index` NULL, so the walk stops AT it and never reaches its source.
///
/// `$seed` is how the seed row is matched against `$2`: `"= $2"` for one
/// execution (the default), `"= ANY($2)"` for a set (the pruner's
/// [`readable_from_any`]).
macro_rules! up_and_root {
    () => {
        up_and_root!("= $2")
    };
    ($seed:literal) => {
        concat!(
            "\
  up(id, parent, loop_index) AS ( \
    SELECT id, parent_execution_id, loop_index FROM workflow.executions \
     WHERE workspace_id = $1 AND id ",
            $seed,
            " \
    UNION \
    SELECT e.id, e.parent_execution_id, e.loop_index \
      FROM workflow.executions e JOIN up ON e.id = up.parent \
     WHERE up.loop_index IS NOT NULL AND e.workspace_id = $1 \
  ), \
  root(id) AS (SELECT id FROM up WHERE loop_index IS NULL)"
        )
    };
}

/// `root` and every loop iteration under it, at any depth: downward through
/// `parent_execution_id` over rows WITH a `loop_index` only.
macro_rules! family {
    () => {
        "\
  family(id) AS ( \
    SELECT id FROM root \
    UNION \
    SELECT e.id FROM workflow.executions e JOIN family f ON e.parent_execution_id = f.id \
     WHERE e.loop_index IS NOT NULL AND e.workspace_id = $1 \
  )"
    };
}

/// Walk parent_execution_id while the row is a LOOP ITERATION (loop_index IS NOT NULL).
/// A retry replay also has parent_execution_id set, with loop_index NULL
/// (0138's table; worker/replay.rs's module doc), and must NOT be walked through.
///
/// `Error::NotFound` when `execution` is not in `workspace` (or its walk ends
/// nowhere, which only a corrupt cycle of iterations could do).
pub async fn loop_root(conn: &mut PgConnection, workspace: Uuid, execution: Uuid) -> Result<Uuid> {
    let root: Option<Uuid> = sqlx::query_scalar(concat!(
        "WITH RECURSIVE ",
        up_and_root!(),
        " SELECT id FROM root"
    ))
    .bind(workspace)
    .bind(execution)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?;
    root.ok_or_else(|| Error::NotFound(format!("execution {execution}")))
}

/// The loop family of `execution`: loop_root(execution) and every loop iteration under it, at any
/// depth (rows reached downward through parent_execution_id WHERE loop_index IS NOT NULL). Spec §4.4 items 1-2, §8.
///
/// Empty when `execution` is not in `workspace`.
pub async fn loop_family(
    conn: &mut PgConnection,
    workspace: Uuid,
    execution: Uuid,
) -> Result<Vec<Uuid>> {
    sqlx::query_scalar(concat!(
        "WITH RECURSIVE ",
        up_and_root!(),
        ", ",
        family!(),
        " SELECT id FROM family"
    ))
    .bind(workspace)
    .bind(execution)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)
}

/// The readable-set query of [`readable_executions`], one text for one
/// execution (`$seed` = `"= $2"`) and for a set (`"= ANY($2)"`,
/// [`readable_from_any`]). Every step is a `UNION` over its seeds, so the set
/// form answers the union of the single forms.
macro_rules! readable_sql {
    ($seed:literal) => {
        concat!(
            "WITH RECURSIVE ",
            up_and_root!($seed),
            ", ",
            family!(),
            ", \
             chain(id) AS ( \
               SELECT id FROM root \
               UNION \
               SELECT src.id FROM chain c \
                 JOIN workflow.executions l ON l.id = c.id AND l.workspace_id = $1 \
                 JOIN workflow.executions src ON src.workspace_id = $1 \
                  AND ((l.loop_index IS NULL AND src.id = l.parent_execution_id) \
                       OR src.id = l.replayed_from_execution_id) \
             ), \
             triggers(id) AS ( \
               SELECT e.trigger_execution_id FROM workflow.executions e \
                WHERE e.workspace_id = $1 AND e.trigger_execution_id IS NOT NULL \
                  AND (e.id ",
            $seed,
            " OR e.id IN (SELECT id FROM chain)) \
             ), \
             trigger_up(id, parent, loop_index) AS ( \
               SELECT id, parent_execution_id, loop_index FROM workflow.executions \
                WHERE workspace_id = $1 AND id IN (SELECT id FROM triggers) \
               UNION \
               SELECT e.id, e.parent_execution_id, e.loop_index \
                 FROM workflow.executions e JOIN trigger_up u ON e.id = u.parent \
                WHERE u.loop_index IS NOT NULL AND e.workspace_id = $1 \
             ), \
             trigger_family(id) AS ( \
               SELECT id FROM trigger_up WHERE loop_index IS NULL \
               UNION \
               SELECT e.id FROM workflow.executions e JOIN trigger_family f \
                   ON e.parent_execution_id = f.id \
                WHERE e.loop_index IS NOT NULL AND e.workspace_id = $1 \
             ) \
             SELECT id FROM family UNION SELECT id FROM chain UNION SELECT id FROM triggers \
             UNION SELECT id FROM trigger_family"
        )
    };
}

/// Spec §4.4: loop_family(execution) (which holds `execution` and its root R)
/// ∪ the replay chain from R — (parent_execution_id WHERE loop_index IS NULL) or replayed_from_execution_id,
///   transitively (a replay copies its source's inputs verbatim, replay.rs:254-257, routes.rs:1943)
/// ∪ trigger_execution_id of `execution`, of R and of every link of the chain (one level each)
/// ∪ the LOOP FAMILY of each such trigger execution: its loop root and every
///   iteration under that root (the user's ruling of 2026-10-01, Task 7
///   concern 1) -- so a `Subflow` child started inside a `Loop` iteration
///   reads the loop root's FILE inputs and its sibling iterations' files, as
///   Kestra OSS allows. Still one trigger level: the trigger's own trigger is
///   never in the set.
///
/// The chain's own loop iterations are NOT in the set: the family is taken
/// of R (and of each trigger) only. Empty when `execution` is not in `workspace`.
pub async fn readable_executions(
    conn: &mut PgConnection,
    workspace: Uuid,
    execution: Uuid,
) -> Result<Vec<Uuid>> {
    sqlx::query_scalar(readable_sql!("= $2"))
        .bind(workspace)
        .bind(execution)
        .fetch_all(&mut *conn)
        .await
        .map_err(db_err)
}

/// The union of [`readable_executions`] over `executions`, in ONE query (the
/// same SQL, seeded with `id = ANY($2)`). The pruner's protected set (spec
/// §10, the user's ruling of 2026-10-04) is this over every execution of
/// `workspace` that has not ended.
pub async fn readable_from_any(
    conn: &mut PgConnection,
    workspace: Uuid,
    executions: &[Uuid],
) -> Result<Vec<Uuid>> {
    if executions.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_scalar(readable_sql!("= ANY($2)"))
        .bind(workspace)
        .bind(executions)
        .fetch_all(&mut *conn)
        .await
        .map_err(db_err)
}

/// THE readability decision. Never reads a row of an execution outside `readable`.
///
/// Per URI: `Malformed` when it does not parse; `NotReadable` when its
/// `{exec}` is not in `readable` -- decided from the URI alone, before and
/// without any query, so the answer cannot depend on whether the file exists;
/// otherwise one `SELECT` over the readable URIs, scoped by `workspace`, and a
/// URI with no row is `Missing` -- or `Pruned` when its `{exec}` (readable)
/// has `files_pruned_at` set (spec §10; a second `SELECT`, only when a URI has
/// no row). The map holds one entry per distinct URI.
///
/// `Pruned`'s `retention_days` is the installed store's
/// (`workflow.storage_retention_days`): the number in the message only, the
/// decision is the row's. With no store installed it is the default, 30.
pub async fn resolve_readable_many(
    conn: &mut PgConnection,
    workspace: Uuid,
    readable: &[Uuid],
    uris: &[String],
) -> Result<BTreeMap<String, std::result::Result<IndexedFile, Unreadable>>> {
    let readable: BTreeSet<Uuid> = readable.iter().copied().collect();
    let mut out = BTreeMap::new();
    let mut lookup: Vec<&str> = Vec::new();
    for u in uris {
        match uri::parse(u) {
            Err(reason) => {
                out.insert(
                    u.clone(),
                    Err(Unreadable::Malformed {
                        uri: u.clone(),
                        reason,
                    }),
                );
            }
            Ok(p) if !readable.contains(&p.execution) => {
                out.insert(u.clone(), Err(Unreadable::NotReadable { uri: u.clone() }));
            }
            Ok(_) => {
                out.insert(u.clone(), Err(Unreadable::Missing { uri: u.clone() }));
                lookup.push(u.as_str());
            }
        }
    }
    if lookup.is_empty() {
        return Ok(out);
    }
    let rows: Vec<(String, Uuid, Option<Uuid>, i64)> = sqlx::query_as(
        "SELECT uri, execution_id, task_run_id, size FROM workflow.execution_files \
          WHERE workspace_id = $1 AND uri = ANY($2)",
    )
    .bind(workspace)
    .bind(&lookup)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    for (uri, execution_id, task_run_id, size) in rows {
        let size = u64::try_from(size)
            .map_err(|_| Error::Internal(anyhow::anyhow!("{uri}: negative size {size}")))?;
        out.insert(
            uri.clone(),
            Ok(IndexedFile {
                uri,
                execution_id,
                task_run_id,
                size,
            }),
        );
    }
    // A readable URI with no row: was its execution's storage pruned?
    let missing: BTreeMap<&str, Uuid> = out
        .iter()
        .filter(|(_, v)| matches!(v, Err(Unreadable::Missing { .. })))
        .filter_map(|(u, _)| Some((u.as_str(), uri::parse(u).ok()?.execution)))
        .collect();
    if missing.is_empty() {
        return Ok(out);
    }
    let executions: Vec<Uuid> = missing
        .values()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let pruned: BTreeSet<Uuid> = sqlx::query_scalar(
        "SELECT id FROM workflow.executions \
          WHERE workspace_id = $1 AND id = ANY($2) AND files_pruned_at IS NOT NULL",
    )
    .bind(workspace)
    .bind(&executions)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?
    .into_iter()
    .collect();
    if pruned.is_empty() {
        return Ok(out);
    }
    let retention_days = super::current().map_or(DEFAULT_RETENTION_DAYS, |f| f.retention_days);
    let gone: Vec<String> = missing
        .into_iter()
        .filter(|(_, e)| pruned.contains(e))
        .map(|(u, _)| u.to_string())
        .collect();
    for uri in gone {
        out.insert(
            uri.clone(),
            Err(Unreadable::Pruned {
                uri,
                retention_days,
            }),
        );
    }
    Ok(out)
}

/// **The readability decision of a READ** (`read()` and the other render
/// functions through the render snapshot, a task staging a URI, a FILE input
/// given as a string): [`resolve_readable_many`] over `readable`, plus the
/// files a waited-for `Subflow` child **published** to an execution of
/// `readable` (package #6 Task B4, spec D10).
///
/// Kestra lets a parent read its child's file outputs
/// (`{{ read(outputs.sub.outputs.f) }}`; OSS's `checkAllowedNamespace`
/// allows every namespace), and egret's readable set does not reach down to a
/// `Subflow` child. So a URI [`resolve_readable_many`] answers
/// [`Unreadable::NotReadable`] is admitted when, and only when, all of these
/// hold:
///
/// 1. it is the string value of one of the child's flow outputs, as copied
///    onto a `Subflow` run of an execution in `readable`
///    (`task_runs.outputs.outputs`, written by `worker::subflow`'s copy alone);
/// 2. that run really started that child: the child row's
///    `trigger_execution_id` and `trigger_run_ix` name the run, and its id is
///    the run's `outputs.executionId` (a task that merely WROTE such a map,
///    a script's outputs, names no child of its own);
/// 3. the URI's `{exec}` is in the child's own loop family (the child and its
///    iterations): the same family rule a FILE output is held to, so a
///    child cannot hand its parent a third execution's file through a STRING
///    output.
///
/// Only the URI is admitted, never the child's other files. An admitted URI
/// then resolves exactly as a readable one does (`Missing`, `Pruned`). Not
/// used by [`super::inputs::check_file_outputs`] (an output publishes only
/// a file of its own loop family, D10) nor by the files API (whose caller's
/// authority is over the execution the route names).
pub async fn resolve_readable_for_read(
    conn: &mut PgConnection,
    workspace: Uuid,
    readable: &[Uuid],
    uris: &[String],
) -> Result<BTreeMap<String, std::result::Result<IndexedFile, Unreadable>>> {
    let mut out = resolve_readable_many(conn, workspace, readable, uris).await?;
    let refused: Vec<&str> = out
        .iter()
        .filter(|(_, v)| matches!(v, Err(Unreadable::NotReadable { .. })))
        .map(|(u, _)| u.as_str())
        .collect();
    if refused.is_empty() || readable.is_empty() {
        return Ok(out);
    }
    let published: Vec<(String, Uuid)> = sqlx::query_as(
        "SELECT DISTINCT o.value #>> '{}', c.id \
           FROM workflow.task_runs tr \
           JOIN workflow.executions c \
             ON c.workspace_id = $1 \
            AND c.trigger_execution_id = tr.execution_id \
            AND c.trigger_run_ix = tr.run_ix \
            AND c.id::text = tr.outputs ->> 'executionId' \
          CROSS JOIN LATERAL jsonb_each( \
                CASE WHEN jsonb_typeof(tr.outputs -> 'outputs') = 'object' \
                     THEN tr.outputs -> 'outputs' ELSE '{}'::jsonb END) o \
          WHERE tr.execution_id = ANY($2) \
            AND jsonb_typeof(o.value) = 'string' \
            AND o.value #>> '{}' = ANY($3)",
    )
    .bind(workspace)
    .bind(readable)
    .bind(&refused)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    let mut admitted: BTreeMap<Uuid, Vec<String>> = BTreeMap::new();
    for (u, child) in published {
        // Parsed already: only a URI that parsed can be `NotReadable`.
        let Ok(p) = uri::parse(&u) else { continue };
        let in_family = p.execution == child
            || match loop_root(conn, workspace, p.execution).await {
                Ok(root) => root == child,
                // Not an execution of this workspace: not the child's.
                Err(Error::NotFound(_)) => false,
                Err(e) => return Err(e),
            };
        if in_family {
            admitted.entry(p.execution).or_default().push(u);
        }
    }
    for (exec, uris) in admitted {
        out.extend(resolve_readable_many(conn, workspace, &[exec], &uris).await?);
    }
    Ok(out)
}

/// `workflow.storage_retention_days`'s default, named by
/// [`Unreadable::Pruned`] when no store is installed in this process.
const DEFAULT_RETENTION_DAYS: u32 = 30;

/// `readable_executions` + `resolve_readable_many`, one element.
pub async fn resolve_readable(
    conn: &mut PgConnection,
    workspace: Uuid,
    current_execution: Uuid,
    uri: &str,
) -> Result<std::result::Result<IndexedFile, Unreadable>> {
    let readable = readable_executions(conn, workspace, current_execution).await?;
    let uri = uri.to_string();
    let mut map =
        resolve_readable_many(conn, workspace, &readable, std::slice::from_ref(&uri)).await?;
    Ok(map
        .remove(&uri)
        .expect("resolve_readable_many answers every URI it is given"))
}
