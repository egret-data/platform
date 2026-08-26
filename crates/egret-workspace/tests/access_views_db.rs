//! Workspace-scope role-catalog view (read-only). Asserts
//! `AccessViewsRepo::list_workspace_roles_with_perms` returns each
//! workspace-scoped role with its direct permission keys, each enriched with a
//! non-empty description + group from the code-defined catalog. DB-gated;
//! self-skips without DATABASE_URL — matching the sibling `members_ax1.rs`
//! harness (same `connect`/`uniq` setup).

use egret_metastore::Db;
use egret_workspace::access_views::AccessViewsRepo;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4().simple())
}

/// A workspace-scope role with two direct permissions surfaces both keys, and
/// each permission carries a non-empty catalog description + group.
#[tokio::test]
async fn workspace_roles_include_direct_permissions() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let repo = AccessViewsRepo::new(db.clone());

    // Seed a custom workspace-scope role with two real catalog permissions.
    let role_name = uniq("Report Reader");
    let role_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) \
         VALUES ('workspace', $1, 'custom read-only') RETURNING id",
    )
    .bind(&role_name)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    for key in ["chart:read", "dashboard:read"] {
        sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
            .bind(role_id)
            .bind(key)
            .execute(&db.pool)
            .await
            .unwrap();
    }

    let roles = repo.list_workspace_roles_with_perms().await.unwrap();
    let seeded = roles
        .iter()
        .find(|r| r.id == role_id)
        .expect("seeded workspace role must be listed");
    assert_eq!(seeded.name, role_name);

    let keys: Vec<&str> = seeded.permissions.iter().map(|p| p.key.as_str()).collect();
    assert_eq!(
        keys,
        vec!["chart:read", "dashboard:read"],
        "both direct permission keys, ordered by key"
    );
    for perm in &seeded.permissions {
        assert!(
            !perm.description.is_empty(),
            "permission {} must have a catalog description",
            perm.key
        );
        assert!(
            !perm.group.is_empty(),
            "permission {} must have a catalog group",
            perm.key
        );
    }

    // A platform-scope role must NOT leak into the workspace catalog.
    assert!(
        !roles.iter().any(|r| r.name == "platform_admin"),
        "platform-scope roles are excluded"
    );
}
