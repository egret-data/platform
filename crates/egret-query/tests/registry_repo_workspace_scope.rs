//! Workspace-scoped DatabaseRepo tests (Epic A: DB connections per-workspace).
//! DB-gated; self-skips without `DATABASE_URL`.
//!
//! These prove the per-workspace repo methods enforce ownership (ADR-19):
//!   - `create_in_workspace(ws_a)` rows are visible to `list_for_workspace(ws_a)`
//!     and ABSENT from `list_for_workspace(ws_b)`,
//!   - `get_in_workspace(ws_a, id)` succeeds, `get_in_workspace(ws_b, id)` is
//!     `NotFound` (IDOR closed),
//!   - `update_in_workspace(ws_b, id)` is `NotFound` and mutates nothing, while
//!     `update_in_workspace(ws_a, id)` applies,
//!   - `delete_in_workspace(ws_b, id)` is `NotFound` (no delete), while
//!     `delete_in_workspace(ws_a, id)` removes the row.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-database --test repo_workspace_scope -- --test-threads=1

use egret_auth::users::UserRepo;
use egret_core::ids::UserId;
use egret_metastore::Db;
use egret_query::registry::model::{ConnOptions, DatabaseInput, Engine};
use egret_query::{DatabaseError, DatabaseRepo};
use egret_workspace::workspaces::WorkspaceRepo;
use uuid::Uuid;

struct Fixture {
    repo: DatabaseRepo,
    ws_a: Uuid,
    ws_b: Uuid,
    creator: UserId,
}

async fn setup() -> Option<Fixture> {
    let dsn = std::env::var("DATABASE_URL").ok()?;

    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secrets = egret_secrets::SecretStore::new(db.clone(), Some(crypto));
    let workspace_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets.clone());
    let repo = DatabaseRepo::new(db.clone(), secrets, workspace_secrets);

    let users = UserRepo::new(db.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let creator = users
        .insert_local(&format!("dbowner-{suffix}@egret.test"), "DB Owner", "x")
        .await
        .expect("seed owner");

    let workspaces = WorkspaceRepo::new(db.clone());
    let ws_a = workspaces
        .create(&format!("dbwsa{suffix}"), "DB WS A", creator)
        .await
        .expect("seed workspace A");
    let ws_b = workspaces
        .create(&format!("dbwsb{suffix}"), "DB WS B", creator)
        .await
        .expect("seed workspace B");

    Some(Fixture {
        repo,
        ws_a: ws_a.id.0,
        ws_b: ws_b.id.0,
        creator,
    })
}

fn input(name: &str) -> DatabaseInput {
    DatabaseInput {
        name: name.to_string(),
        engine: Engine::Trino,
        host: "trino.local".to_string(),
        port: 8080,
        options: ConnOptions::default(),
        secret_id: None,
        new_secret: None,
    }
}

#[tokio::test]
async fn workspace_scoped_repo_enforces_ownership() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let name = format!("conn-{}", Uuid::new_v4().simple());

    // CREATE in workspace A.
    let created = f
        .repo
        .create_in_workspace(f.ws_a, &input(&name), None, Some(f.creator))
        .await
        .expect("create in ws A");
    let id = created.id;

    // LIST — visible in A, absent from B.
    let in_a = f.repo.list_for_workspace(f.ws_a).await.expect("list A");
    assert!(
        in_a.iter().any(|d| d.id == id),
        "created conn must be listed for its own workspace"
    );
    let in_b = f.repo.list_for_workspace(f.ws_b).await.expect("list B");
    assert!(
        in_b.iter().all(|d| d.id != id),
        "conn must NOT be listed for another workspace"
    );

    // GET — A ok, B NotFound (IDOR closed).
    let got = f
        .repo
        .get_in_workspace(f.ws_a, id)
        .await
        .expect("get in ws A");
    assert_eq!(got.id, id);
    assert!(
        matches!(
            f.repo.get_in_workspace(f.ws_b, id).await,
            Err(DatabaseError::NotFound(_))
        ),
        "get from another workspace must be NotFound"
    );

    // UPDATE from B → NotFound, and nothing changes.
    let mut foreign = input(&name);
    foreign.host = "attacker.local".to_string();
    assert!(
        matches!(
            f.repo
                .update_in_workspace(f.ws_b, id, &foreign, None, Some(f.creator))
                .await,
            Err(DatabaseError::NotFound(_))
        ),
        "update from another workspace must be NotFound"
    );
    let unchanged = f.repo.get_in_workspace(f.ws_a, id).await.expect("get A");
    assert_eq!(
        unchanged.host, "trino.local",
        "a foreign update must not mutate the row"
    );

    // UPDATE from A → applies.
    let mut owned = input(&name);
    owned.host = "trino-2.local".to_string();
    owned.port = 9090;
    let updated = f
        .repo
        .update_in_workspace(f.ws_a, id, &owned, None, Some(f.creator))
        .await
        .expect("update in ws A");
    assert_eq!(updated.host, "trino-2.local");
    assert_eq!(updated.port, 9090);

    // DELETE from B → NotFound, row survives.
    assert!(
        matches!(
            f.repo.delete_in_workspace(f.ws_b, id).await,
            Err(DatabaseError::NotFound(_))
        ),
        "delete from another workspace must be NotFound"
    );
    assert!(
        f.repo.get_in_workspace(f.ws_a, id).await.is_ok(),
        "a foreign delete must not remove the row"
    );

    // DELETE from A → removed.
    f.repo
        .delete_in_workspace(f.ws_a, id)
        .await
        .expect("delete in ws A");
    assert!(
        matches!(
            f.repo.get_in_workspace(f.ws_a, id).await,
            Err(DatabaseError::NotFound(_))
        ),
        "owner delete must remove the row"
    );
}
