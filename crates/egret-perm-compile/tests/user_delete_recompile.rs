//! Username takeover, spec §4.3: `UserRepo::hard_delete` recompiles every
//! profile that named the deleted user, in the delete's own transaction —
//! through their ROLES, their WORKSPACE memberships and the CLEARANCES their
//! roles satisfy, not only their groups and engine grants.
//!
//! Before the fix the delete recompiled the group and engine-grant arms only,
//! so a compiled body kept naming the deleted user's username until something
//! else recompiled it. The ledger (0209) makes such a stale name match nobody
//! from then on; this keeps the bodies correct as well. Every assertion asks
//! the engine (`egret-ranger-eval` over the profile's download) as the deleted
//! user's username, read right after `hard_delete` returns, with no recompile
//! in between.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test user_delete_recompile

#[path = "support/engine_eval.rs"]
mod engine_eval;

use egret_auth::users::UserRepo;
use egret_core::UserId;
use egret_metastore::Db;
use egret_perm::{
    Action, Effect, Grant, GrantOrigin, PermStore, Principal, PrincipalKind, PrincipalSet, Scope,
};
use egret_perm_compile::{recompile_service, PermGroupRecompiler};
use egret_policy::PolicyStore;
use engine_eval::{allowed, starrocks_decides};
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

struct Fixture {
    db: Db,
    perm: PermStore,
    policy: PolicyStore,
    service_id: i64,
    workspace_id: Uuid,
    warehouse_id: Uuid,
    catalog_id: Uuid,
    catalog_name: String,
}

/// A StarRocks profile with one engine (egret's default image) bound to it and
/// one registry catalog attached, the shape `clearance_starrocks.rs` uses.
async fn seed(db: &Db) -> Fixture {
    let pool = &db.pool;
    let perm = PermStore::new(pool.clone());
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks service-def");
    let service = policy
        .create_service(&unique("sr_udel"), "starrocks", &serde_json::json!({}))
        .await
        .expect("create service");
    let workspace_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("udel-ws"))
            .bind("User Delete WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-udel-test','AKIATEST') RETURNING id",
    )
    .bind(unique("udel-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("udel_wh"))
    .bind(workspace_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret',$3,$2) RETURNING id",
    )
    .bind(unique("udel-eng"))
    .bind(service.id)
    .bind(serde_json::json!({
        "starrocksConfig": {"feImage": egret_perm::DEFAULT_ENGINE_IMAGES.starrocks_fe_image}
    }))
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog_name = unique("udel_cat");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&catalog_name)
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("registry catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach catalog");
    Fixture {
        db: db.clone(),
        perm,
        policy,
        service_id: service.id,
        workspace_id,
        warehouse_id,
        catalog_id,
        catalog_name,
    }
}

async fn new_user(db: &Db, username: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) VALUES ('local',$1,'T',$2) RETURNING id",
    )
    .bind(format!("{username}@udel.test"))
    .bind(username)
    .fetch_one(&db.pool)
    .await
    .expect("insert user")
}

async fn new_role(db: &Db, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .expect("insert role")
}

async fn access_grant(db: &Db, user_id: Uuid, role_id: Uuid, workspace: Option<Uuid>) {
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(role_id)
    .bind(if workspace.is_some() {
        "workspace"
    } else {
        "platform"
    })
    .bind(workspace)
    .execute(&db.pool)
    .await
    .expect("access grant");
}

fn p(kind: PrincipalKind, r: &str) -> Principal {
    Principal {
        kind,
        r#ref: r.to_string(),
    }
}

fn grant(f: &Fixture, effect: Effect, include: Vec<Principal>, exclude: Vec<Principal>) -> Grant {
    Grant {
        id: Uuid::nil(),
        effect,
        actions: vec![Action::Read],
        scope: Scope {
            catalog_id: f.catalog_id,
            schema: None,
            table: None,
            column: None,
        },
        principals: PrincipalSet { include, exclude },
        origin: GrantOrigin::Explicit,
    }
}

fn actor(id: Uuid) -> egret_core::Principal {
    egret_core::Principal {
        user_id: UserId(id),
        email: "admin@udel.test".into(),
        platform_roles: vec!["platform_admin".into()],
        permissions: vec![],
        group_ids: vec![],
        impersonator: None,
    }
}

async fn delete(f: &Fixture, user: Uuid) {
    let admin = new_user(&f.db, &unique("udel_admin")).await;
    UserRepo::new(f.db.clone())
        .hard_delete(UserId(user), &actor(admin), &PermGroupRecompiler)
        .await
        .expect("delete")
        .expect("the user existed");
}

/// `select` on a column of the fixture's catalog, as `user`.
async fn can_select(f: &Fixture, user: &str) -> bool {
    allowed(
        &starrocks_decides(
            &f.db,
            f.service_id,
            user,
            "select",
            &[
                ("catalog", f.catalog_name.as_str()),
                ("database", "sales"),
                ("table", "orders"),
                ("column", "email"),
            ],
        )
        .await,
    )
}

async fn recompile(f: &Fixture) {
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");
}

#[tokio::test]
async fn deleting_a_role_holder_recompiles_the_role_grant() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let role = unique("analyst");
    let role_id = new_role(&db, &role).await;
    let eve_name = unique("eve");
    let eve = new_user(&db, &eve_name).await;
    access_grant(&db, eve, role_id, None).await;
    f.perm
        .create_grant(&grant(
            &f,
            Effect::Allow,
            vec![p(PrincipalKind::Role, &role)],
            vec![],
        ))
        .await
        .expect("grant");
    recompile(&f).await;
    assert!(can_select(&f, &eve_name).await, "the arrangement");

    delete(&f, eve).await;
    assert!(
        !can_select(&f, &eve_name).await,
        "the deleted holder's username still decides an allow in the compiled role grant"
    );
}

#[tokio::test]
async fn deleting_a_workspace_member_recompiles_the_members_grant() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let ws_role = new_role(&db, &unique("ws_viewer")).await;
    let eve_name = unique("eve");
    let eve = new_user(&db, &eve_name).await;
    access_grant(&db, eve, ws_role, Some(f.workspace_id)).await;
    f.perm
        .create_grant(&grant(
            &f,
            Effect::Allow,
            vec![p(
                PrincipalKind::WorkspaceMembers,
                &f.workspace_id.to_string(),
            )],
            vec![],
        ))
        .await
        .expect("grant");
    recompile(&f).await;
    assert!(can_select(&f, &eve_name).await, "the arrangement");

    delete(&f, eve).await;
    assert!(
        !can_select(&f, &eve_name).await,
        "the deleted member's username still decides an allow in the compiled members grant"
    );
}

/// The fail-open direction: a role in a deny's EXCLUDE compiles to an
/// exception naming the holder. Left stale, the exception keeps excusing
/// whoever carries that username from the deny.
#[tokio::test]
async fn deleting_a_role_holder_recompiles_a_deny_exclusion() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let role = unique("exempt");
    let role_id = new_role(&db, &role).await;
    let eve_name = unique("eve");
    let eve = new_user(&db, &eve_name).await;
    access_grant(&db, eve, role_id, None).await;
    let public = p(PrincipalKind::Group, egret_perm::PUBLIC_GROUP);
    f.perm
        .create_grant(&grant(&f, Effect::Allow, vec![public.clone()], vec![]))
        .await
        .expect("allow");
    f.perm
        .create_grant(&grant(
            &f,
            Effect::Deny,
            vec![public],
            vec![p(PrincipalKind::Role, &role)],
        ))
        .await
        .expect("deny");
    recompile(&f).await;
    assert!(
        can_select(&f, &eve_name).await,
        "the arrangement: the holder is excused"
    );

    delete(&f, eve).await;
    assert!(
        !can_select(&f, &eve_name).await,
        "the deleted holder's username is still excused from the deny"
    );
}

/// A clearance (`classification_policies.required_roles`) compiles to a deny
/// per tagged column whose exceptions name the role's holders.
#[tokio::test]
async fn deleting_a_cleared_role_holder_recompiles_the_clearance() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let role = unique("cleared");
    let role_id = new_role(&db, &role).await;
    let eve_name = unique("eve");
    let eve = new_user(&db, &eve_name).await;
    access_grant(&db, eve, role_id, None).await;
    let tag: Uuid = sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1,$2) RETURNING id")
        .bind(unique("udel-tag"))
        .bind(unique("UdelTag"))
        .fetch_one(&db.pool)
        .await
        .expect("tag");
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
         column_name) VALUES ($1,$2,'sales','orders','email')",
    )
    .bind(tag)
    .bind(f.warehouse_id)
    .execute(&db.pool)
    .await
    .expect("tag column");
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, ARRAY[$2], ARRAY[]::uuid[])",
    )
    .bind(tag)
    .bind(&role)
    .execute(&db.pool)
    .await
    .expect("gate");
    f.perm
        .create_grant(&grant(
            &f,
            Effect::Allow,
            vec![p(PrincipalKind::Group, egret_perm::PUBLIC_GROUP)],
            vec![],
        ))
        .await
        .expect("allow");
    recompile(&f).await;
    assert!(
        can_select(&f, &eve_name).await,
        "the arrangement: the holder is cleared"
    );

    delete(&f, eve).await;
    assert!(
        !can_select(&f, &eve_name).await,
        "the deleted holder's username is still cleared for the classified column"
    );
}
