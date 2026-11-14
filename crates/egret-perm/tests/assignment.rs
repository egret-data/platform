//! `sync_assignment_grants`: assigning a catalog to a workspace grants that
//! workspace `read` (SP4a Task 6). Since SP3c-3 Task 5 the grant is the explicit
//! `ws:<slug> members` GROUP shape (`origin='workspace_assignment_group'`,
//! principal `kind='group'`), not the old `workspace_assignment` +
//! `workspace_members` shape — see `crates/egret-perm/src/assignment.rs`.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm --test assignment

use egret_metastore::Db;
use egret_perm::{
    ws_members_group_name, Action, Effect, Grant, GrantOrigin, PermStore, Principal, PrincipalKind,
    PrincipalSet, Scope,
};
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

async fn seed_catalog_and_workspace(pool: &sqlx::PgPool) -> (Uuid, Uuid, String) {
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) \
         VALUES ($1, 'generic', '{}'::jsonb) RETURNING id",
    )
    .bind(unique("perm_assign"))
    .fetch_one(pool)
    .await
    .expect("catalog");

    let slug = unique("perm-assign-ws");
    let workspace_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, status) VALUES ($1, 'Perm Assign', 'active') \
         RETURNING id",
    )
    .bind(&slug)
    .fetch_one(pool)
    .await
    .expect("workspace");

    (catalog_id, workspace_id, slug)
}

fn explicit_write_grant(catalog_id: Uuid) -> Grant {
    Grant {
        id: Uuid::nil(),
        effect: Effect::Allow,
        actions: vec![Action::Write],
        scope: Scope {
            catalog_id,
            schema: None,
            table: None,
            column: None,
        },
        principals: PrincipalSet {
            include: vec![Principal {
                kind: PrincipalKind::User,
                r#ref: "admin".into(),
            }],
            exclude: vec![],
        },
        origin: GrantOrigin::Explicit,
    }
}

/// `PermStore::list_grants` has no `WHERE` clause (`store.rs`'s own tests
/// only ever match by exact id, never by a raw total count) — this crate's
/// test binaries share ONE process-private database (`egret-test-db`'s doc
/// comment), and `#[tokio::test]` functions inside one binary run
/// concurrently, so a bare `list_grants().len()` counts every OTHER test's
/// rows too. Scoping to `catalog_id` is what makes an assertion here about
/// "this test's grants" rather than "the whole table right now" — the
/// brief's literal Step 2 code omits this and is racy under this workspace's
/// shared-process-DB test design; it failed exactly this way (2 vs 1) when
/// run concurrently before this scoping was added.
async fn grants_for(store: &PermStore, catalog_id: Uuid) -> Vec<Grant> {
    store
        .list_grants()
        .await
        .unwrap()
        .into_iter()
        .filter(|g| g.scope.catalog_id == catalog_id)
        .collect()
}

#[tokio::test]
async fn assigning_a_catalog_creates_exactly_one_read_grant_for_that_workspace() {
    let Some(db) = test_db().await else { return };
    let store = PermStore::new(db.pool.clone());
    let (catalog_id, ws, slug) = seed_catalog_and_workspace(&db.pool).await;
    let group = ws_members_group_name(&slug);

    egret_perm::sync_assignment_grants(&store, catalog_id, &[ws])
        .await
        .unwrap();
    let g: Vec<_> = grants_for(&store, catalog_id)
        .await
        .into_iter()
        .filter(|g| matches!(g.origin, GrantOrigin::WorkspaceAssignmentGroup { .. }))
        .collect();
    assert_eq!(g.len(), 1);
    assert!(
        matches!(g[0].origin, GrantOrigin::WorkspaceAssignmentGroup { workspace_id } if workspace_id == ws),
        "the assignment grant names its workspace"
    );
    assert_eq!(
        g[0].actions,
        vec![Action::Read],
        "an assignment grants READ, never write"
    );
    assert_eq!(
        g[0].principals.include,
        vec![Principal {
            kind: PrincipalKind::Group,
            r#ref: group.clone(),
        }],
        "SP3c-3: the assignment grant's principal is the managed `ws:<slug> members` group"
    );

    // Idempotent: re-running must not accumulate. The unique index is the
    // enforcement; this asserts the code path agrees with it — and does so
    // by identity and principal-row survival, not just a count: a
    // delete-then-reinsert-without-principals implementation would also
    // leave the count at 1, but would change the id and lose the principal.
    egret_perm::sync_assignment_grants(&store, catalog_id, &[ws])
        .await
        .unwrap();
    let after: Vec<_> = grants_for(&store, catalog_id)
        .await
        .into_iter()
        .filter(|g| matches!(g.origin, GrantOrigin::WorkspaceAssignmentGroup { .. }))
        .collect();
    assert_eq!(after.len(), 1);
    assert_eq!(
        after[0].id, g[0].id,
        "a re-run must not delete-and-reinsert the same (catalog, workspace) grant"
    );
    assert_eq!(
        after[0].principals.include,
        vec![Principal {
            kind: PrincipalKind::Group,
            r#ref: group,
        }],
        "the principal row must still be there after a no-op re-run"
    );
}

#[tokio::test]
async fn unassigning_removes_the_implicit_grant_but_never_an_explicit_one() {
    let Some(db) = test_db().await else { return };
    let store = PermStore::new(db.pool.clone());
    let (catalog_id, ws, _slug) = seed_catalog_and_workspace(&db.pool).await;
    egret_perm::sync_assignment_grants(&store, catalog_id, &[ws])
        .await
        .unwrap();
    let explicit = store
        .create_grant(&explicit_write_grant(catalog_id))
        .await
        .unwrap();

    egret_perm::sync_assignment_grants(&store, catalog_id, &[])
        .await
        .unwrap();
    let left = grants_for(&store, catalog_id).await;
    assert_eq!(left.len(), 1, "the explicit grant must survive");
    assert_eq!(left[0].id, explicit);
}

#[tokio::test]
async fn reassigning_to_a_different_workspace_drops_the_old_implicit_grant() {
    // Exercises the DELETE branch on a non-empty new set, which the brief's
    // two tests don't reach: `unassigning_...` only exercises `workspace_ids
    // = []`, and `assigning_a_catalog_...` never changes the set after the
    // first sync.
    let Some(db) = test_db().await else { return };
    let store = PermStore::new(db.pool.clone());
    let (catalog_id, ws1, _slug1) = seed_catalog_and_workspace(&db.pool).await;
    let (_, ws2, _slug2) = seed_catalog_and_workspace(&db.pool).await;

    egret_perm::sync_assignment_grants(&store, catalog_id, &[ws1])
        .await
        .unwrap();
    egret_perm::sync_assignment_grants(&store, catalog_id, &[ws2])
        .await
        .unwrap();

    let g = grants_for(&store, catalog_id).await;
    assert_eq!(
        g.len(),
        1,
        "the old workspace's implicit grant must be gone"
    );
    assert!(matches!(
        g[0].origin,
        GrantOrigin::WorkspaceAssignmentGroup { workspace_id } if workspace_id == ws2
    ));
}
