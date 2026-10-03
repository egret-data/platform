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
macro_rules! up_and_root {
    () => {
        "\
  up(id, parent, loop_index) AS ( \
    SELECT id, parent_execution_id, loop_index FROM workflow.executions \
     WHERE workspace_id = $1 AND id = $2 \
    UNION \
    SELECT e.id, e.parent_execution_id, e.loop_index \
      FROM workflow.executions e JOIN up ON e.id = up.parent \
     WHERE up.loop_index IS NOT NULL AND e.workspace_id = $1 \
  ), \
  root(id) AS (SELECT id FROM up WHERE loop_index IS NULL)"
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
    sqlx::query_scalar(concat!(
        "WITH RECURSIVE ",
        up_and_root!(),
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
              AND (e.id = $2 OR e.id IN (SELECT id FROM chain)) \
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
    ))
    .bind(workspace)
    .bind(execution)
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
