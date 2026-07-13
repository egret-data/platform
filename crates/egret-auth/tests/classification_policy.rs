//! DB-gated e2e integration test for tag-conditioned object access (Access P3.3).
//! Self-skips without DATABASE_URL.
//!
//! Exercises the full classification-forbid decision path: `load_inner` compiles
//! a `forbid` for a gated tag, `authorize` resolves the object's tag keys and the
//! subject's role names into `context.roles`, and Cedar's forbid-beats-permit
//! denies a tagged object unless the subject holds a cleared role (or is a
//! platform_admin, always auto-exempt).
//!
//! Model: `crates/egret-auth/tests/object_scope.rs`.

use egret_auth::pdp::CedarPdp;
use egret_core::authz::{ObjectRef, Pdp, Resource, Subject};
use egret_core::{UserId, WorkspaceId};
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

async fn seed_workspace(db: &Db) -> WorkspaceId {
    let id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, 'W') RETURNING id")
            .bind(format!("ws-{}", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    WorkspaceId(id)
}

/// Grant `role_id` to user `u` at workspace scope on `ws`.
async fn grant_ws(db: &Db, u: UserId, role_id: Uuid, ws: WorkspaceId) {
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'workspace', $3)",
    )
    .bind(u.0)
    .bind(role_id)
    .bind(ws.0)
    .execute(&db.pool)
    .await
    .unwrap();
}

fn dashboard(id: Uuid, ws: WorkspaceId) -> Resource {
    Resource::Object(ObjectRef {
        kind: "dashboard".into(),
        id,
        folder_id: None,
        workspace_id: ws,
    })
}

#[tokio::test]
async fn classification_forbid_gates_tagged_object() {
    let Some(db) = connect().await else {
        eprintln!("skip: DATABASE_URL unset");
        return;
    };

    let action = "dashboard:read";

    // The permit role: seeded `ws_editor` genuinely holds dashboard:read; a ws
    // grant cascades to any object in the workspace.
    let ws_editor_id: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = 'ws_editor'")
            .fetch_one(&db.pool)
            .await
            .expect("seeded ws role `ws_editor` must exist");

    // The clearance role: a bespoke PLATFORM role that carries NO permissions —
    // it exists only so a grant of it lands the name in `context.roles`. Platform
    // scope both models an org-wide clearance and sidesteps the one-ws-role-per-
    // member partial unique index (so U can hold ws_editor AND the clearance).
    // Created BEFORE the PDP build so its name is compiled into `role_names`.
    let cleared_name = format!("finance_cleared_{}", Uuid::new_v4().simple());
    let cleared_id: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) VALUES ('platform', $1, 'clearance') \
         RETURNING id",
    )
    .bind(&cleared_name)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    // A gated tag `confidential` + its classification policy (requires the
    // clearance role). Tag a dashboard D1; leave D2 untagged (parity control).
    let tag_key = format!("confidential-{}", Uuid::new_v4().simple());
    let tag_id: Uuid =
        sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, 'Confidential') RETURNING id")
            .bind(&tag_key)
            .fetch_one(&db.pool)
            .await
            .unwrap();

    let w = seed_workspace(&db).await;
    let d1 = Uuid::new_v4(); // tagged confidential
    let d2 = Uuid::new_v4(); // untagged

    sqlx::query(
        "INSERT INTO tag_assignments (tag_id, object_type, object_id) VALUES ($1, 'dashboard', $2)",
    )
    .bind(tag_id)
    .bind(d1)
    .execute(&db.pool)
    .await
    .unwrap();

    sqlx::query("INSERT INTO classification_policies (tag_id, required_roles) VALUES ($1, $2)")
        .bind(tag_id)
        .bind(vec![cleared_name.clone()])
        .execute(&db.pool)
        .await
        .unwrap();

    // Build the PDP AFTER seeding so `load_inner` compiles the classification forbid.
    let bus = egret_events::EventBus::new();
    let pdp = CedarPdp::build(db.clone(), &bus).await.unwrap();

    // ── Subject U: holds ws_editor (permit) but NOT the clearance role ───────
    let u = seed_user(&db).await;
    grant_ws(&db, u, ws_editor_id, w).await;
    let subject = Subject {
        user_id: u,
        group_ids: vec![],
    };

    // (deny) tagged D1 without clearance → forbid wins over the ws_editor permit.
    let deny = pdp
        .authorize(&subject, action, &dashboard(d1, w))
        .await
        .unwrap();
    assert!(
        !deny.allow,
        "tagged object must be DENIED without the cleared role"
    );

    // (parity) untagged D2 with the same permit → allowed (forbid `when` is false).
    let untagged = pdp
        .authorize(&subject, action, &dashboard(d2, w))
        .await
        .unwrap();
    assert!(
        untagged.allow,
        "untagged object must remain allowed (classification forbid does not fire)"
    );

    // (allow) grant the clearance role at PLATFORM scope → context.roles now
    // lists it → the classification forbid's `unless` is satisfied → allow.
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL)",
    )
    .bind(u.0)
    .bind(cleared_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let allow = pdp
        .authorize(&subject, action, &dashboard(d1, w))
        .await
        .unwrap();
    assert!(
        allow.allow,
        "tagged object must be ALLOWED once the subject holds the cleared role"
    );

    // ── platform_admin is auto-exempt (never locked out of a tagged object) ──
    let admin = seed_user(&db).await;
    let platform_admin_role_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM roles WHERE scope = 'platform' AND name = 'platform_admin'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("seeded platform role `platform_admin` must exist");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL)",
    )
    .bind(admin.0)
    .bind(platform_admin_role_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let admin_subject = Subject {
        user_id: admin,
        group_ids: vec![],
    };
    let admin_allow = pdp
        .authorize(&admin_subject, action, &dashboard(d1, w))
        .await
        .unwrap();
    assert!(
        admin_allow.allow,
        "platform_admin must NEVER be locked out of a tagged object"
    );

    eprintln!(
        "classification_forbid_gates_tagged_object OK: deny={}, untagged-allow={}, cleared-allow={}, admin-allow={}",
        !deny.allow, untagged.allow, allow.allow, admin_allow.allow
    );
}

/// FIX 1b (defense in depth): a classification row whose `required_roles` somehow
/// holds a Cedar-breaking value (inserted RAW here, bypassing `set_policy`'s
/// FIX-1a validation) must NOT fail the whole PDP reload — `load_inner` LOGS and
/// SKIPS that one row, so the PDP still boots and every OTHER gate keeps working.
/// One poisoned row must never DoS authz / fail-open the process.
#[tokio::test]
async fn poisoned_classification_row_is_skipped_not_fatal() {
    let Some(db) = connect().await else {
        eprintln!("skip: DATABASE_URL unset");
        return;
    };

    let action = "dashboard:read";
    let ws_editor_id: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = 'ws_editor'")
            .fetch_one(&db.pool)
            .await
            .expect("seeded ws role `ws_editor` must exist");

    // A GOOD gated tag on D_good (valid clearance role) — must still enforce.
    let good_key = format!("good-{}", Uuid::new_v4().simple());
    let good_tag: Uuid =
        sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, 'Good') RETURNING id")
            .bind(&good_key)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let cleared_name = format!("finance_cleared_{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO roles (scope, name, description) VALUES ('platform', $1, 'clearance')",
    )
    .bind(&cleared_name)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO classification_policies (tag_id, required_roles) VALUES ($1, $2)")
        .bind(good_tag)
        .bind(vec![cleared_name.clone()])
        .execute(&db.pool)
        .await
        .unwrap();

    // A POISONED gated tag on D_bad — required_roles holds a Cedar-injection value.
    // Inserted RAW (no repo validation) to simulate a legacy / hand-edited row.
    let bad_key = format!("bad-{}", Uuid::new_v4().simple());
    let bad_tag: Uuid =
        sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, 'Bad') RETURNING id")
            .bind(&bad_key)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO classification_policies (tag_id, required_roles) VALUES ($1, $2)")
        .bind(bad_tag)
        .bind(vec![
            "x\"]) || true || context.roles.containsAny([\"y".to_string()
        ])
        .execute(&db.pool)
        .await
        .unwrap();

    let w = seed_workspace(&db).await;
    let d_good = Uuid::new_v4();
    let d_bad = Uuid::new_v4();
    for (tag, obj) in [(good_tag, d_good), (bad_tag, d_bad)] {
        sqlx::query(
            "INSERT INTO tag_assignments (tag_id, object_type, object_id) VALUES ($1, 'dashboard', $2)",
        )
        .bind(tag)
        .bind(obj)
        .execute(&db.pool)
        .await
        .unwrap();
    }

    // The PDP must BUILD despite the poisoned row (FIX 1b: skip-and-log).
    let bus = egret_events::EventBus::new();
    let pdp = CedarPdp::build(db.clone(), &bus)
        .await
        .expect("PDP must load even with a poisoned classification row");

    // A ws_editor (no clearance) is denied the GOOD tagged object → the surviving
    // gate still enforces.
    let u = seed_user(&db).await;
    grant_ws(&db, u, ws_editor_id, w).await;
    let subject = Subject {
        user_id: u,
        group_ids: vec![],
    };
    let good_decision = pdp
        .authorize(&subject, action, &dashboard(d_good, w))
        .await
        .unwrap();
    assert!(
        !good_decision.allow,
        "the valid classification gate must still enforce after skipping the poisoned row"
    );

    // The BAD tagged object's gate was SKIPPED (inert), so the ws_editor permit
    // stands → allowed. The point is the reload did not fail; this row is simply
    // ungated until corrected.
    let bad_decision = pdp
        .authorize(&subject, action, &dashboard(d_bad, w))
        .await
        .unwrap();
    assert!(
        bad_decision.allow,
        "the poisoned row's gate is inert (skipped), so the underlying permit stands"
    );

    eprintln!(
        "poisoned_classification_row_is_skipped_not_fatal OK: good-denied={}, bad-inert-allowed={}",
        !good_decision.allow, bad_decision.allow
    );
}
