//! DB-gated security test (Effort B / B1b-2 Task 1): the admin-graph `forbid`
//! makes authz-graph-mutating actions NON-delegatable. Only `platform_admin`
//! may perform them; a non-admin is denied EVEN IF a role they hold was
//! force-granted the action (the adversarial escalation case), because a Cedar
//! `forbid` overrides every `permit`.
//!
//! Self-skips without DATABASE_URL. Model: `crates/egret-auth/tests/cedar_pdp.rs`.

use egret_auth::pdp::CedarPdp;
use egret_core::authz::{Pdp, Resource, Subject};
use egret_core::rbac::{self, Scope};
use egret_core::UserId;
use egret_metastore::Db;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn seed_user(db: &Db) -> UserId {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local', $1, 'T') RETURNING id",
    )
    .bind(format!("{}@test.local", Uuid::new_v4()))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    UserId(id)
}

async fn platform_role_id(db: &Db, name: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'platform' AND name = $1")
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .unwrap_or_else(|_| panic!("seeded platform role `{name}` must exist"))
}

/// Assign a platform role to a user (mirrored into `access_grants` by trigger).
async fn assign_platform_role(db: &Db, user: UserId, role_id: Uuid) {
    sqlx::query(
        "INSERT INTO platform_role_assignments (principal_type, principal_id, role_id) \
         VALUES ('user', $1, $2)",
    )
    .bind(user.0)
    .bind(role_id)
    .execute(&db.pool)
    .await
    .unwrap();
}

/// The user's direct group memberships, as the `Subject` sees them.
async fn groups_of(db: &Db, user: UserId) -> Vec<egret_core::ids::GroupId> {
    let rows: Vec<(Uuid,)> = sqlx::query_as("SELECT group_id FROM user_groups WHERE user_id = $1")
        .bind(user.0)
        .fetch_all(&db.pool)
        .await
        .unwrap();
    rows.into_iter()
        .map(|(g,)| egret_core::ids::GroupId(g))
        .collect()
}

#[tokio::test]
async fn admin_graph_actions_are_non_escalatable() {
    let Some(db) = connect().await else {
        eprintln!("skip: DATABASE_URL unset");
        return;
    };

    // ── ADMIN: the seeded platform_admin role at platform scope. ──
    let admin = seed_user(&db).await;
    assign_platform_role(&db, admin, platform_role_id(&db, "platform_admin").await).await;

    // ── USER: a fresh platform role R holding EVERY grantable platform
    //    permission, at platform scope. A legitimate, maximally-privileged
    //    non-admin. ──
    let user = seed_user(&db).await;
    let r_id: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) \
         VALUES ('platform', $1, 'B1b-2 forbid test role') RETURNING id",
    )
    .bind(format!("forbid_test_{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let platform_keys: Vec<&'static str> = rbac::PERMISSIONS
        .iter()
        .filter(|p| p.scope == Scope::Platform)
        .map(|p| p.key)
        .collect();
    for key in &platform_keys {
        sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
            .bind(r_id)
            .bind(key)
            .execute(&db.pool)
            .await
            .unwrap();
    }

    // ── ADVERSARIAL: force-inject an admin-graph action into R, bypassing the
    //    catalog (`is_known` would reject it). If the `forbid` were instead a
    //    fragile "never-grantable string" invariant, this row would escalate
    //    USER to role-creation. The `forbid` must still deny it. ──
    sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, 'role:create')")
        .bind(r_id)
        .execute(&db.pool)
        .await
        .unwrap();

    assign_platform_role(&db, user, r_id).await;

    // Build the PDP AFTER all seeding, so its templates compile from the rows
    // above (including the adversarial `role:create` grant on R).
    let bus = egret_events::EventBus::new();
    let pdp = CedarPdp::build(db.clone(), &bus).await.unwrap();

    let admin_subject = Subject {
        user_id: admin,
        group_ids: groups_of(&db, admin).await,
    };
    let user_subject = Subject {
        user_id: user,
        group_ids: groups_of(&db, user).await,
    };

    for a in rbac::ADMIN_GRAPH_ACTIONS {
        let admin_dec = pdp
            .authorize(&admin_subject, a, &Resource::Platform)
            .await
            .unwrap();
        assert!(admin_dec.allow, "platform_admin denied admin-graph `{a}`");

        let user_dec = pdp
            .authorize(&user_subject, a, &Resource::Platform)
            .await
            .unwrap();
        assert!(
            !user_dec.allow,
            "non-admin ALLOWED admin-graph `{a}` — privilege escalation!"
        );
    }

    // Sanity: the `forbid` is surgical — USER keeps a normal platform permission
    // its role legitimately holds. If this flipped to deny, the forbid is too
    // broad and would break ordinary decisions.
    let normal = pdp
        .authorize(&user_subject, "engine:read", &Resource::Platform)
        .await
        .unwrap();
    assert!(
        normal.allow,
        "non-admin must still be allowed a normal held permission (engine:read)"
    );

    eprintln!(
        "admin_graph_forbid OK: all {} admin-graph actions admin-allowed AND \
         non-admin-denied (incl. force-granted role:create); engine:read still allowed",
        rbac::ADMIN_GRAPH_ACTIONS.len()
    );
}
