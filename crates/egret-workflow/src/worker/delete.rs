//! **Delete an execution** -- workflow package 7d (spec
//! `docs/en/specs/2026-11-16-workflow-delete-labels-logs-design.md` Q1,
//! Q2, Q5, D1-D3; Kestra `ExecutionService.delete`, `ES:747-767`).
//!
//! One transaction, the root's row locked `FOR UPDATE` first, then every
//! family member's (the root and every `parent_execution_id` descendant:
//! loop iterations and retry replacements, which `ON DELETE CASCADE` takes
//! with the root). `FOR UPDATE` conflicts with the `KEY SHARE` an FK check
//! takes, so no execution can start to point at a locked member while this
//! runs: a replay, a subflow child or an iteration created now waits, then
//! fails its own insert. Every refusal comes before the first write but
//! one: the S8 detach, which a later refusal rolls back with the rest.
//!
//! **The family is locked to a fixpoint.** The recursive read that finds
//! the family runs under `READ COMMITTED` before its rows are locked, so a
//! child being inserted under a member that is not yet locked (a retry
//! replacement's wake holds that member's row and inserts the next
//! replacement, needing only `KEY SHARE` on it, not on the root) is not
//! seen. The `FOR UPDATE` on that member waits for the insert to commit;
//! the read is then run again and any new id locked, until a read finds
//! nothing that is not locked yet. From then on no member can gain a
//! child, so the family the checks, the objects and the counts use is the
//! one the cascade deletes.
//!
//! Not `claim::load`: nothing is settled, and an execution whose frozen
//! source no longer plans must still be deletable (plan S5).
//!
//! Objects first, then rows (the pruner's order, `storage/prune.rs`): a
//! store failure is an `Err`, the transaction rolls back, every row stays,
//! and the call can be repeated. A member whose prefix is not provably its
//! own (`own_prefix` is `None`) keeps its objects, logged, as the pruner
//! does. Each member's `_wd/` snapshots are deleted too (plan S4).
//!
//! The store is not transactional: a failure after some objects are gone
//! (the store failing on member k of N, or the final `DELETE` or the commit
//! failing after every object was deleted) rolls back the rows but not the
//! objects, so rows can remain whose files are already gone. Repeating the
//! call is the recovery: deleting an absent prefix succeeds, and the rows
//! then go.

use egret_core::{Error, Result};
use sqlx::{PgConnection, PgPool, Row};
use uuid::Uuid;

use super::kill::{is_deadlock, DEADLOCK_RETRIES};
use crate::storage::prune::own_prefix;
use crate::storage::FileStore;

/// What to delete: an execution of `workspace`, and with `delete_storage`
/// its stored files too.
#[derive(Clone, Copy, Debug)]
pub struct DeleteRequest {
    pub workspace: Uuid,
    pub id: Uuid,
    pub delete_storage: bool,
}

/// What a delete removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deleted {
    pub namespace: String,
    pub flow_key: String,
    /// The root's state when it was deleted.
    pub state: String,
    /// The family's size: the root and every `parent_execution_id` descendant.
    pub executions: u64,
    /// `workflow.execution_files` rows of the family (D4's `files`).
    pub files: i64,
    /// S8: Flow-trigger fires whose `trigger_execution_id` was set to NULL.
    pub detached: u64,
}

/// Why a delete wrote nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeleteRefused {
    /// D2: a loop iteration or retry replacement; its family's root.
    NotARoot { root: Uuid },
    /// Q1: this family member is not idle; its state.
    NotIdle { execution: Uuid, state: String },
    /// S9 (decided (a)): a Flow-trigger event of the family is still unprocessed.
    ListenersPending,
    /// Q2: `Subflow` children, replays and restarts into a new execution
    /// outside the family that point at it (at most `MAX_NAMED_DEPENDENTS`,
    /// oldest first), and how many there are. Flow-trigger fires are
    /// detached instead (S8).
    HasDependents { executions: Vec<Uuid>, total: i64 },
}

/// At most this many dependents are named in a refusal.
pub const MAX_NAMED_DEPENDENTS: i64 = 20;

/// **Delete an ended root execution** and its family (module doc).
///
/// `fs`: the store the objects are deleted from (`storage::current()` in
/// production; a test's own store). `None`, or `delete_storage: false`,
/// deletes no object. `Err(NotFound)` for a missing or foreign id.
pub async fn delete_execution(
    pool: &PgPool,
    fs: Option<&FileStore>,
    req: DeleteRequest,
) -> Result<std::result::Result<Deleted, DeleteRefused>> {
    let mut retries = 0;
    loop {
        match delete_once(pool, fs, req).await {
            Err(e) if is_deadlock(&e) && retries < DEADLOCK_RETRIES => {
                retries += 1;
                tracing::info!(execution = %req.id, "a delete was a deadlock's victim; running it again");
            }
            other => return other,
        }
    }
}

/// One family member: what its prefix is made of.
struct Member {
    id: Uuid,
    namespace: String,
    flow_key: String,
}

/// What [`screen`] found: the root's row, its locked family, and how many
/// Flow-trigger fires it detached (in the still-open transaction).
struct Screened {
    namespace: String,
    flow_key: String,
    state: String,
    family: Vec<Member>,
    ids: Vec<Uuid>,
    detached: u64,
}

/// **Every refusal of a delete**, in the write's order, inside `tx`: the
/// root's row locked, D2 (only a root), the family locked to a fixpoint, Q1
/// (every member idle), S9 (listeners heard) and Q2 (no dependents), with the
/// S8 detach before Q2. [`delete_once`] runs it, then deletes;
/// [`delete_would_refuse`] runs the SAME function and rolls back, so a bulk
/// pre-flight and the write cannot disagree.
async fn screen(
    tx: &mut PgConnection,
    ws: Uuid,
    id: Uuid,
) -> Result<std::result::Result<Screened, DeleteRefused>> {
    // 1. The root's row, locked; scoped by the workspace.
    let row = sqlx::query(
        "SELECT namespace, flow_key, state, parent_execution_id \
           FROM workflow.executions WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
    )
    .bind(id)
    .bind(ws)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?
    .ok_or_else(|| Error::NotFound("execution".into()))?;
    // 2. D2: only a root.
    if row.get::<Option<Uuid>, _>("parent_execution_id").is_some() {
        let root = root_of(tx, ws, id).await?;
        return Ok(Err(DeleteRefused::NotARoot { root }));
    }
    // 3. The family, locked in id order.
    let family = family(tx, ws, id).await?;
    let ids: Vec<Uuid> = family.iter().map(|m| m.id).collect();
    // 4. Q1: every member idle (the root first when it is the busy one).
    let busy: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, state FROM workflow.executions \
          WHERE id = ANY($1) AND NOT COALESCE(workflow.execution_is_idle(id), false) \
          ORDER BY (id = $2) DESC, id LIMIT 1",
    )
    .bind(&ids)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?;
    if let Some((execution, state)) = busy {
        return Ok(Err(DeleteRefused::NotIdle { execution, state }));
    }
    // 5. S9 (user decision (a)): its Flow triggers have not all heard its end.
    let pending: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM workflow.flow_trigger_events \
                         WHERE source_execution_id = ANY($1))",
    )
    .bind(&ids)
    .fetch_one(&mut *tx)
    .await
    .map_err(db_err)?;
    if pending {
        return Ok(Err(DeleteRefused::ListenersPending));
    }
    // 6. S8 (user decision (b)): a Flow-trigger fire is detached, not a
    // dependent. `flow_trigger_event_id` is set exactly on a fire
    // (migration 0157's CHECK); 0138's CHECKs accept the NULL. The fire
    // keeps its `execution_trigger` record, which names the source as data.
    // A refusal below rolls this back.
    let detached = sqlx::query(
        "UPDATE workflow.executions SET trigger_execution_id = NULL \
          WHERE workspace_id = $1 AND trigger_execution_id = ANY($2) \
            AND flow_trigger_event_id IS NOT NULL",
    )
    .bind(ws)
    .bind(&ids)
    .execute(&mut *tx)
    .await
    .map_err(db_err)?
    .rows_affected();
    // 7. Q2: what still points at the family from outside it: `Subflow`
    // children and replays/restarts into a new execution (the fires no
    // longer do, after step 6).
    let (total, executions) = dependents(tx, ws, &ids).await?;
    if total > 0 {
        return Ok(Err(DeleteRefused::HasDependents { executions, total }));
    }
    Ok(Ok(Screened {
        namespace: row.get("namespace"),
        flow_key: row.get("flow_key"),
        state: row.get("state"),
        family,
        ids,
        detached,
    }))
}

/// **Would a delete of `id` be refused?** Read-only: runs [`screen`], the
/// write's own refusal rule, in a transaction that is rolled back (the S8
/// detach it does on the way is undone). `Err(NotFound)` for a missing or
/// foreign id.
pub async fn delete_would_refuse(
    pool: &PgPool,
    workspace: Uuid,
    id: Uuid,
) -> Result<Option<DeleteRefused>> {
    let mut tx = pool.begin().await.map_err(db_err)?;
    let refused = screen(&mut tx, workspace, id).await?.err();
    tx.rollback().await.map_err(db_err)?;
    Ok(refused)
}

async fn delete_once(
    pool: &PgPool,
    fs: Option<&FileStore>,
    req: DeleteRequest,
) -> Result<std::result::Result<Deleted, DeleteRefused>> {
    let ws = req.workspace;
    let mut tx = pool.begin().await.map_err(db_err)?;
    let Screened {
        namespace,
        flow_key,
        state,
        family,
        ids,
        detached,
    } = match screen(&mut tx, ws, req.id).await? {
        Ok(s) => s,
        Err(refused) => return Ok(Err(refused)),
    };
    // 8. D4's counts, before the rows go.
    let files: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.execution_files \
          WHERE workspace_id = $1 AND execution_id = ANY($2)",
    )
    .bind(ws)
    .bind(&ids)
    .fetch_one(&mut *tx)
    .await
    .map_err(db_err)?;
    // 9. D3: the objects, member by member.
    if let (true, Some(fs)) = (req.delete_storage, fs) {
        for m in &family {
            match own_prefix(fs, ws, &m.namespace, &m.flow_key, m.id) {
                Some((store_prefix, _)) => delete_prefix(fs, m.id, &store_prefix).await?,
                None => tracing::warn!(
                    workspace = %ws,
                    execution = %m.id,
                    "workflow delete: this execution's prefix is not provably its own; \
                     its stored files are kept"
                ),
            }
            delete_prefix(fs, m.id, &fs.working_dir_root(ws, m.id)).await?;
        }
    }
    // 10. The rows: the cascade takes the family, its runs, logs, index
    // rows, loop values and outbox rows.
    sqlx::query("DELETE FROM workflow.executions WHERE id = $1 AND workspace_id = $2")
        .bind(req.id)
        .bind(ws)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            // Unreachable while step 3's locks hold (see the module doc);
            // a 409, not a 500, if a future writer breaks that.
            if e.as_database_error()
                .is_some_and(|d| d.is_foreign_key_violation())
            {
                Error::Conflict(format!(
                    "execution {} gained a dependent while it was being deleted; try again",
                    req.id
                ))
            } else {
                db_err(e)
            }
        })?;
    tx.commit().await.map_err(db_err)?;
    Ok(Ok(Deleted {
        namespace,
        flow_key,
        state,
        executions: family.len() as u64,
        files,
        detached,
    }))
}

/// The top of `id`'s `parent_execution_id` chain, whatever `loop_index`
/// says (a retry replacement's parent may itself be an iteration).
async fn root_of(conn: &mut PgConnection, ws: Uuid, id: Uuid) -> Result<Uuid> {
    sqlx::query_scalar(
        "WITH RECURSIVE up(id, parent) AS ( \
           SELECT id, parent_execution_id FROM workflow.executions \
            WHERE id = $1 AND workspace_id = $2 \
           UNION \
           SELECT e.id, e.parent_execution_id FROM up \
             JOIN workflow.executions e ON e.id = up.parent AND e.workspace_id = $2) \
         SELECT id FROM up WHERE parent IS NULL",
    )
    .bind(id)
    .bind(ws)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?
    .ok_or_else(|| Error::NotFound(format!("the root of execution {id}")))
}

/// The root and every `parent_execution_id` descendant (plan S2: not
/// `storage::index::loop_family`, which stops at rows without a
/// `loop_index`), each row locked `FOR UPDATE`, in id order within each
/// pass, read again until no unlocked member is left (the module doc's
/// fixpoint). Returned in id order.
async fn family(conn: &mut PgConnection, ws: Uuid, root: Uuid) -> Result<Vec<Member>> {
    let mut locked: Vec<Member> = Vec::new();
    loop {
        let ids: Vec<Uuid> = sqlx::query_scalar(
            "WITH RECURSIVE fam(id) AS ( \
               SELECT $1::uuid \
               UNION \
               SELECT e.id FROM workflow.executions e JOIN fam f ON e.parent_execution_id = f.id \
                WHERE e.workspace_id = $2) \
             SELECT id FROM fam",
        )
        .bind(root)
        .bind(ws)
        .fetch_all(&mut *conn)
        .await
        .map_err(db_err)?;
        let new: Vec<Uuid> = ids
            .into_iter()
            .filter(|id| !locked.iter().any(|m| m.id == *id))
            .collect();
        // Every member read is locked: none can gain a child any more. (A
        // row deleted between the read and its lock is not locked, and the
        // next read no longer lists it.)
        if new.is_empty() {
            break;
        }
        let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
            "SELECT id, namespace, flow_key FROM workflow.executions \
              WHERE workspace_id = $1 AND id = ANY($2) ORDER BY id FOR UPDATE",
        )
        .bind(ws)
        .bind(&new)
        .fetch_all(&mut *conn)
        .await
        .map_err(db_err)?;
        locked.extend(rows.into_iter().map(|(id, namespace, flow_key)| Member {
            id,
            namespace,
            flow_key,
        }));
    }
    locked.sort_by_key(|m| m.id);
    Ok(locked)
}

/// How many executions outside `family` point at a member, and the first
/// `MAX_NAMED_DEPENDENTS` of them, oldest first. Both columns are indexed
/// (0138:245, 0166:72). Called after the S8 detach, so a Flow-trigger fire
/// is not among them.
async fn dependents(
    conn: &mut PgConnection,
    ws: Uuid,
    family: &[Uuid],
) -> Result<(i64, Vec<Uuid>)> {
    let rows: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT id, count(*) OVER () FROM workflow.executions \
          WHERE workspace_id = $1 AND NOT (id = ANY($2)) \
            AND (trigger_execution_id = ANY($2) OR replayed_from_execution_id = ANY($2)) \
          ORDER BY created_at, id LIMIT $3",
    )
    .bind(ws)
    .bind(family)
    .bind(MAX_NAMED_DEPENDENTS)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    let total = rows.first().map_or(0, |(_, n)| *n);
    Ok((total, rows.into_iter().map(|(id, _)| id).collect()))
}

async fn delete_prefix(fs: &FileStore, execution: Uuid, prefix: &str) -> Result<()> {
    fs.store.delete_prefix(prefix).await.map(drop).map_err(|e| {
        Error::Internal(anyhow::anyhow!(
            "deleting the stored files of execution {execution} failed, so nothing was deleted: {e}"
        ))
    })
}

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::Error::new(e))
}
