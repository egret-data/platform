//! DB-gated `ImpersonationRepo` tests. Self-skips without `DATABASE_URL` ONLY under
//! `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS; CI / `just up` provides Postgres.

use egret_auth::impersonation::{ImpersonationRepo, NewImpersonation};
use egret_auth::password::hash_password;
use egret_auth::users::UserRepo;
use egret_core::{UserId, WorkspaceId};
use egret_metastore::Db;

#[path = "support/recorder.rs"]
mod recorder;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique_email(tag: &str) -> String {
    format!("{tag}-{}@test.local", uuid::Uuid::new_v4())
}

async fn seed_user(db: &Db) -> UserId {
    let repo = UserRepo::new(db.clone());
    let hash = hash_password("pw").unwrap();
    repo.insert_local(&unique_email("imp"), "Imp", &hash)
        .await
        .unwrap()
}

/// Seed a local user with a *specific* email (the overlay tests assert on it).
/// Cleans up any pre-existing user with that email first (cascades sessions +
/// impersonations) so the tests are re-runnable against a persistent DB.
async fn seed_user_named(db: &Db, email: &str) -> UserId {
    sqlx::query("DELETE FROM users WHERE email = $1")
        .bind(email)
        .execute(&db.pool)
        .await
        .unwrap();
    let repo = UserRepo::new(db.clone());
    let hash = hash_password("pw").unwrap();
    repo.insert_local(email, "Named", &hash).await.unwrap()
}

/// The `sessions.id` for a raw token, resolved via the same `hash_token` the
/// service stores (`token_hash`).
async fn session_id_for_token(db: &Db, token: &str) -> uuid::Uuid {
    sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM sessions WHERE token_hash = $1")
        .bind(egret_auth::token::hash_token(token))
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

/// Seed a workspace owned by `owner`; returns its id.
async fn seed_workspace(db: &Db, owner: UserId) -> WorkspaceId {
    let slug = format!("imp-ws-{}", uuid::Uuid::new_v4());
    let id = sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO workspaces (slug, name, created_by) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(&slug)
    .bind("Imp WS")
    .bind(owner.0)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    WorkspaceId(id)
}

async fn seed_session(db: &Db, user: UserId) -> uuid::Uuid {
    sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO sessions (token_hash, user_id, absolute_expires_at, idle_expires_at) \
         VALUES ($1, $2, now() + interval '1 day', now() + interval '1 day') RETURNING id",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(user.0)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn start_lookup_exit_roundtrip() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let repo = ImpersonationRepo::new(db.clone());
    let admin = seed_user(&db).await;
    let target = seed_user(&db).await;
    let sess = seed_session(&db, admin).await;

    repo.start(NewImpersonation {
        admin_session_id: sess,
        impersonator_id: admin,
        target_user_id: target,
        workspace_scope: None,
        reason: "debugging".into(),
        ttl_minutes: 30,
    })
    .await
    .unwrap();

    let active = repo
        .active_for_session(sess)
        .await
        .unwrap()
        .expect("active");
    assert_eq!(active.target_user_id, target.0);
    assert_eq!(active.impersonator_id, admin.0);

    repo.exit(sess).await.unwrap();
    assert!(repo.active_for_session(sess).await.unwrap().is_none());
}

#[tokio::test]
async fn expired_overlay_is_not_active_and_is_cleaned() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let repo = ImpersonationRepo::new(db.clone());
    let admin = seed_user(&db).await;
    let target = seed_user(&db).await;
    let sess = seed_session(&db, admin).await;

    repo.start(NewImpersonation {
        admin_session_id: sess,
        impersonator_id: admin,
        target_user_id: target,
        workspace_scope: None,
        reason: "x".into(),
        ttl_minutes: 30,
    })
    .await
    .unwrap();
    sqlx::query(
        "UPDATE impersonations SET expires_at = now() - interval '1 minute' WHERE admin_session_id = $1",
    )
    .bind(sess)
    .execute(&db.pool)
    .await
    .unwrap();

    assert!(
        repo.active_for_session(sess).await.unwrap().is_none(),
        "an expired overlay must not be active"
    );
    // And confirm it was reaped:
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM impersonations WHERE admin_session_id = $1")
            .bind(sess)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        remaining, 0,
        "expired overlay should be deleted by the lookup"
    );
}

#[tokio::test]
async fn start_twice_on_one_session_is_rejected() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let repo = ImpersonationRepo::new(db.clone());
    let admin = seed_user(&db).await;
    let t1 = seed_user(&db).await;
    let t2 = seed_user(&db).await;
    let sess = seed_session(&db, admin).await;
    let n = |t: UserId| NewImpersonation {
        admin_session_id: sess,
        impersonator_id: admin,
        target_user_id: t,
        workspace_scope: None,
        reason: "x".into(),
        ttl_minutes: 30,
    };
    repo.start(n(t1)).await.unwrap();
    assert!(
        repo.start(n(t2)).await.is_err(),
        "unique index blocks a second overlay"
    );
}

/// `start` must not depend on `active_for_session` having run first to reap a
/// dead overlay: a session whose overlay merely EXPIRED (never explicitly
/// exited) must accept a fresh `start` on its own.
#[tokio::test]
async fn start_reaps_expired_overlay_on_same_session() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let repo = ImpersonationRepo::new(db.clone());
    let admin = seed_user(&db).await;
    let t1 = seed_user(&db).await;
    let t2 = seed_user(&db).await;
    let sess = seed_session(&db, admin).await;

    repo.start(NewImpersonation {
        admin_session_id: sess,
        impersonator_id: admin,
        target_user_id: t1,
        workspace_scope: None,
        reason: "x".into(),
        ttl_minutes: 30,
    })
    .await
    .unwrap();
    sqlx::query(
        "UPDATE impersonations SET expires_at = now() - interval '1 minute' WHERE admin_session_id = $1",
    )
    .bind(sess)
    .execute(&db.pool)
    .await
    .unwrap();

    // No `active_for_session` call in between — `start` alone must reap the
    // dead row and succeed with a different target.
    repo.start(NewImpersonation {
        admin_session_id: sess,
        impersonator_id: admin,
        target_user_id: t2,
        workspace_scope: None,
        reason: "y".into(),
        ttl_minutes: 30,
    })
    .await
    .expect("a fresh start must succeed once the old overlay has expired");

    let active = repo
        .active_for_session(sess)
        .await
        .unwrap()
        .expect("active");
    assert_eq!(active.target_user_id, t2.0);
}

#[tokio::test]
async fn validate_resolves_target_principal_under_overlay() {
    let Some(db) = connect().await else { return };
    let sessions = egret_auth::session::SessionService::new(
        db.clone(),
        chrono::Duration::hours(8),
        chrono::Duration::hours(1),
    );
    let imp = egret_auth::impersonation::ImpersonationRepo::new(db.clone());

    let admin = seed_user_named(&db, "admin@x").await;
    let target = seed_user_named(&db, "target@x").await;
    let issued = sessions.create(admin, "ua", "ip").await.unwrap();
    let sess_id = session_id_for_token(&db, &issued.token).await;

    // No overlay yet → admin.
    let p = sessions.validate(&issued.token).await.unwrap().unwrap();
    assert_eq!(p.email, "admin@x");
    assert!(p.impersonator.is_none());

    imp.start(egret_auth::impersonation::NewImpersonation {
        admin_session_id: sess_id,
        impersonator_id: admin,
        target_user_id: target,
        workspace_scope: None,
        reason: "x".into(),
        ttl_minutes: 30,
    })
    .await
    .unwrap();

    // Now → target, with impersonator = admin.
    let p = sessions.validate(&issued.token).await.unwrap().unwrap();
    assert_eq!(p.email, "target@x");
    assert_eq!(p.user_id, target);
    let imper = p.impersonator.expect("impersonator set");
    assert_eq!(imper.user_id, admin);
    assert!(imper.workspace_scope.is_none());
}

#[tokio::test]
async fn validate_ignores_expired_overlay_and_returns_admin() {
    let Some(db) = connect().await else { return };
    let sessions = egret_auth::session::SessionService::new(
        db.clone(),
        chrono::Duration::hours(8),
        chrono::Duration::hours(1),
    );
    let imp = egret_auth::impersonation::ImpersonationRepo::new(db.clone());
    let admin = seed_user_named(&db, "adm2@x").await;
    let target = seed_user_named(&db, "tgt2@x").await;
    let issued = sessions.create(admin, "ua", "ip").await.unwrap();
    let sess_id = session_id_for_token(&db, &issued.token).await;
    imp.start(egret_auth::impersonation::NewImpersonation {
        admin_session_id: sess_id,
        impersonator_id: admin,
        target_user_id: target,
        workspace_scope: None,
        reason: "x".into(),
        ttl_minutes: 30,
    })
    .await
    .unwrap();
    sqlx::query("UPDATE impersonations SET expires_at = now() - interval '1 minute' WHERE admin_session_id = $1")
        .bind(sess_id)
        .execute(&db.pool)
        .await
        .unwrap();

    // Expired overlay → back to the admin.
    let p = sessions.validate(&issued.token).await.unwrap().unwrap();
    assert_eq!(p.email, "adm2@x");
    assert!(p.impersonator.is_none());
}

#[tokio::test]
async fn validate_carries_workspace_scope() {
    let Some(db) = connect().await else { return };
    let sessions = egret_auth::session::SessionService::new(
        db.clone(),
        chrono::Duration::hours(8),
        chrono::Duration::hours(1),
    );
    let imp = egret_auth::impersonation::ImpersonationRepo::new(db.clone());
    let admin = seed_user_named(&db, "adm3@x").await;
    let target = seed_user_named(&db, "tgt3@x").await;
    let ws = seed_workspace(&db, admin).await;
    let issued = sessions.create(admin, "ua", "ip").await.unwrap();
    let sess_id = session_id_for_token(&db, &issued.token).await;
    imp.start(egret_auth::impersonation::NewImpersonation {
        admin_session_id: sess_id,
        impersonator_id: admin,
        target_user_id: target,
        workspace_scope: Some(ws),
        reason: "x".into(),
        ttl_minutes: 30,
    })
    .await
    .unwrap();

    let p = sessions.validate(&issued.token).await.unwrap().unwrap();
    assert_eq!(p.impersonator.unwrap().workspace_scope, Some(ws));
}

/// A disabled target must not resolve: the overlay is inert and validate returns
/// the admin principal (impersonating a suspended user is a no-op).
#[tokio::test]
async fn validate_disabled_target_falls_through_to_admin() {
    let Some(db) = connect().await else { return };
    let sessions = egret_auth::session::SessionService::new(
        db.clone(),
        chrono::Duration::hours(8),
        chrono::Duration::hours(1),
    );
    let imp = egret_auth::impersonation::ImpersonationRepo::new(db.clone());
    let admin = seed_user_named(&db, "adm4@x").await;
    let target = seed_user_named(&db, "tgt4@x").await;
    let issued = sessions.create(admin, "ua", "ip").await.unwrap();
    let sess_id = session_id_for_token(&db, &issued.token).await;
    imp.start(egret_auth::impersonation::NewImpersonation {
        admin_session_id: sess_id,
        impersonator_id: admin,
        target_user_id: target,
        workspace_scope: None,
        reason: "x".into(),
        ttl_minutes: 30,
    })
    .await
    .unwrap();
    // Disable the target after the overlay was started.
    sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
        .bind(target.0)
        .execute(&db.pool)
        .await
        .unwrap();

    let p = sessions.validate(&issued.token).await.unwrap().unwrap();
    assert_eq!(
        p.email, "adm4@x",
        "disabled target overlay must not resolve"
    );
    assert!(p.impersonator.is_none());
}

/// Grant `user` a fresh platform-scoped role carrying `policy:update` — the shape
/// migration `0059_rbac_permission_seed.sql` gives `data_steward`, built here with
/// its own role so the fixture does not depend on the seed's contents. It is
/// deliberately NOT `platform_admin`: the pre-existing rule already refused that
/// target under both scopes, so a test built on one would prove nothing new.
async fn grant_platform_steward_shaped_role(db: &Db, user: UserId) {
    let role_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) VALUES ('platform', $1, 'overlay liveness') \
         RETURNING id",
    )
    .bind(format!("overlay_steward_{}", uuid::Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, 'policy:update')")
        .bind(role_id)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL)",
    )
    .bind(user.0)
    .bind(role_id)
    .execute(&db.pool)
    .await
    .unwrap();
}

/// **The platform-authority rule must hold for the LIFE of the overlay, not only at
/// its creation** (whole-branch review, H-1).
///
/// `validate_impersonation_target` reads the target's platform-scoped role grants
/// ONCE, when the overlay is written. `SessionService::validate` then rebuilds the
/// TARGET's `platform_roles` and `permissions` from live tables on **every request**
/// for the overlay's 30 minutes. Nothing re-checked in between, and the production
/// trigger needs no attacker permission: `sync_user_oidc_groups` replaces
/// `user_groups` from the IdP claim on every OIDC login, and group-derived platform
/// grants are admitted by the same query that fills the principal.
///
/// So a `ws_admin` of A started a ws-scoped overlay on a clean member of A; the
/// target later gained a platform role; and the **same admin token** then resolved
/// to a principal carrying `policy:update` with
/// `impersonator.workspace_scope = Some(A)`. Only three consumers in the tree
/// enforce `workspace_scope` and none of them is in `egret-policy`, so
/// `PUT /api/control-plane/policies/{id}` was reachable — and egret IS the Ranger
/// admin, so the body stored there is what another workspace's engine enforces.
///
/// **This test drives the TIME dimension, which IS the defect.** The creation guard
/// passes first (asserted), the overlay then resolves to the target (asserted — so
/// the test cannot pass against a predicate that makes every overlay inert), and
/// only *afterwards* does the target gain the role. A test that granted the role up
/// front would measure the already-working creation guard instead.
#[tokio::test]
async fn an_overlay_goes_inert_when_its_target_gains_a_platform_role_later() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let users = UserRepo::new(db.clone());
    let sessions = egret_auth::session::SessionService::new(
        db.clone(),
        chrono::Duration::hours(8),
        chrono::Duration::hours(1),
    );
    let imp = ImpersonationRepo::new(db.clone());
    let admin = seed_user_named(&db, "overlay-h1-admin@x").await;
    let target = seed_user_named(&db, "overlay-h1-target@x").await;
    let ws = seed_workspace(&db, admin).await;

    // t0: the creation guard PASSES — at this instant the target holds no
    // platform-scoped role grant at all.
    egret_auth::impersonation::validate_impersonation_target(
        &users,
        &sessions,
        admin,
        target,
        "support ticket",
        Some(ws),
    )
    .await
    .expect("a clean target must pass the creation guard, or this test measures nothing");

    let issued = sessions.create(admin, "ua", "ip").await.unwrap();
    let sess_id = session_id_for_token(&db, &issued.token).await;
    imp.start(NewImpersonation {
        admin_session_id: sess_id,
        impersonator_id: admin,
        target_user_id: target,
        workspace_scope: Some(ws),
        reason: "support ticket".into(),
        ttl_minutes: 30,
    })
    .await
    .unwrap();

    // t1: LIVE. The overlay resolves to the target, scoped to A.
    let p = sessions.validate(&issued.token).await.unwrap().unwrap();
    assert_eq!(
        p.email, "overlay-h1-target@x",
        "the overlay must be live BEFORE the grant, or the assertion below is vacuous"
    );
    assert_eq!(
        p.impersonator
            .as_ref()
            .expect("impersonator set")
            .workspace_scope,
        Some(ws)
    );

    // t2: the target gains platform authority mid-overlay — an IdP group change or
    // a console grant, neither of which the admin needs any permission to cause.
    grant_platform_steward_shaped_role(&db, target).await;

    // t3: the SAME admin token must now resolve to the ADMIN. The overlay is inert.
    let p = sessions.validate(&issued.token).await.unwrap().unwrap();
    assert_eq!(
        p.email, "overlay-h1-admin@x",
        "a ws-scoped overlay must go INERT the moment its target holds platform \
         authority — no /api/control-plane route confines a scoped overlay"
    );
    assert_eq!(p.user_id, admin);
    assert!(
        p.impersonator.is_none(),
        "an inert overlay must not report an impersonator either: the three consumers \
         of workspace_scope would otherwise see a scoped admin session"
    );
    assert!(
        !p.permissions.iter().any(|x| x == "policy:update"),
        "the admin token must not acquire the target's platform permission, got {:?}",
        p.permissions
    );

    // The overlay ROW is untouched — inert, not reaped. This is the same shape as a
    // target disabled mid-overlay, and it means the admin is back to being an admin
    // without a destructive write on a read path.
    assert!(
        imp.active_for_session(sess_id).await.unwrap().is_some(),
        "the overlay row must survive: it is treated as inert on the read path, not deleted"
    );
}

/// **Control for the permissive arm, at the new `validate` call site.**
///
/// A **platform-scoped** overlay (`workspace_scope = None`) on a target holding a
/// non-`platform_admin` platform role must keep resolving to the target: the caller
/// on that path is already a `platform_admin`, whose PDP bypass means they hold
/// strictly more than any steward, so refusing it prevents no escalation and
/// removes the support path of reproducing what a steward sees.
///
/// This is the cell a tightening that collapsed the rule to "refuse any platform
/// role" would silently delete, and nothing else in the suite notices a predicate
/// that merely makes MORE overlays inert. `validate` is asserted to hand back the
/// target *with* `policy:update`, so this is the live half of the pair the H-1 test
/// above completes.
#[tokio::test]
async fn a_platform_scoped_overlay_on_a_steward_target_stays_live() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let users = UserRepo::new(db.clone());
    let sessions = egret_auth::session::SessionService::new(
        db.clone(),
        chrono::Duration::hours(8),
        chrono::Duration::hours(1),
    );
    let imp = ImpersonationRepo::new(db.clone());
    let admin = seed_user_named(&db, "overlay-plat-admin@x").await;
    let target = seed_user_named(&db, "overlay-plat-target@x").await;
    grant_platform_steward_shaped_role(&db, target).await;

    // The creation guard permits this pairing on the platform path.
    egret_auth::impersonation::validate_impersonation_target(
        &users,
        &sessions,
        admin,
        target,
        "reproduce a steward's view",
        None,
    )
    .await
    .expect("a platform-scoped overlay MAY target a non-admin platform-permission holder");

    let issued = sessions.create(admin, "ua", "ip").await.unwrap();
    let sess_id = session_id_for_token(&db, &issued.token).await;
    imp.start(NewImpersonation {
        admin_session_id: sess_id,
        impersonator_id: admin,
        target_user_id: target,
        workspace_scope: None,
        reason: "reproduce a steward's view".into(),
        ttl_minutes: 30,
    })
    .await
    .unwrap();

    let p = sessions.validate(&issued.token).await.unwrap().unwrap();
    assert_eq!(
        p.email, "overlay-plat-target@x",
        "a platform-scoped overlay on a steward target must stay LIVE"
    );
    assert!(
        p.impersonator
            .as_ref()
            .expect("impersonator set")
            .workspace_scope
            .is_none(),
        "the platform overlay carries no workspace scope"
    );
    assert!(
        p.permissions.iter().any(|x| x == "policy:update"),
        "the support path exists to see what the steward sees, so the steward's own \
         permission must be present, got {:?}",
        p.permissions
    );
}

/// A service account must not be an impersonation target.
///
/// Migration 0119's `users_service_has_no_password` CHECK exists to make "a
/// service account can never hold an interactive session" structural rather than
/// a property of the login handler. An overlay routes straight around that: it
/// resolves an ADMIN's real session to the target's `Principal`, so no password
/// is ever involved. Left unguarded, `validate_impersonation_target` is the one
/// way to put a machine principal inside a browser session, and the audit trail
/// would then record a human acting as a machine.
///
/// Both legs are asserted — a human target must still validate — so this cannot
/// pass against a guard that refuses every target.
#[tokio::test]
async fn a_service_account_cannot_be_impersonated() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let users = UserRepo::new(db.clone());
    let sessions = egret_auth::session::SessionService::new(
        db.clone(),
        chrono::Duration::hours(8),
        chrono::Duration::hours(1),
    );
    let admin = seed_user(&db).await;

    // A human target passes every other guard in this function, so it isolates
    // the provider check as the only difference below.
    let human = seed_user(&db).await;
    egret_auth::impersonation::validate_impersonation_target(
        &users,
        &sessions,
        admin,
        human,
        "debugging",
        None,
    )
    .await
    .expect("a human target must still be impersonable");

    let svc = users
        .insert_service(&unique_email("svc.reporting"), "reporting")
        .await
        .expect("insert service account");
    let err = egret_auth::impersonation::validate_impersonation_target(
        &users,
        &sessions,
        admin,
        svc,
        "debugging",
        None,
    )
    .await
    .expect_err("a service account must not be impersonable");
    assert!(
        matches!(err, egret_core::Error::InvalidInput(_)),
        "expected InvalidInput, got: {err:?}"
    );
}

/// **SP4d debt Task 13: a target holding ANY platform-scoped role grant is
/// refused, not only a `platform_admin`.**
///
/// The chain this closes, by code path:
///
/// * `egret_workspace::routes::ws_impersonate_start` lets a `ws_admin` of
///   workspace A target any MEMBER of A who is not a `ws_admin` there, and writes
///   an overlay carrying `workspace_scope = Some(A)`.
/// * `egret_workspace::context::WsContext` confines a scoped overlay only on a
///   route that resolves a `{workspace}` path segment — the only thing that builds
///   one. **No `/api/control-plane` route resolves one**, and every handler on all
///   five of those routers gates on a `Scope::Platform` permission instead.
/// * Migration `0059_rbac_permission_seed.sql` seeds the platform `data_steward`
///   role with the whole `policy:*` family, `policy:update` included.
///
/// ⇒ that `ws_admin` could create a permission grant on another workspace's
/// catalog, which the same request compiles into the Ranger artifact that
/// workspace's engine downloads.
///
/// **The fixture is built so it holds the state it guards.** It does not merely
/// insert a role row: it resolves the target's own session through
/// `SessionService::validate` and asserts the resulting `Principal` really carries
/// `policy:update`, because that is the authority the overlay would confer. A
/// fixture that proved only "a row exists" would pass against a guard keyed to
/// something else entirely.
///
/// # All four cells, because the permissive arm is the load-bearing one
///
/// The rule is keyed to the `intended_scope` of the overlay about to be written,
/// not to the target alone, so the table has two dimensions and this test drives
/// every cell:
///
/// | target | `intended_scope = Some(ws)` (ws_admin path) | `intended_scope = None` (platform path) |
/// |---|---|---|
/// | holds NO platform role | **Ok** | **Ok** |
/// | holds a platform `data_steward`-shaped role | **Forbidden** | **Ok** |
///
/// The bottom-right cell is the one a future tightening would silently delete: a
/// `platform_admin` already holds strictly more than any steward (PDP bypass), so
/// refusing that target prevents no escalation and removes the support path of
/// reproducing what a steward sees. Nothing else in this suite notices a guard that
/// merely refuses MORE, which is why it is asserted here rather than left to prose.
///
/// A `platform_admin` target is refused under **both** scopes — the rule that
/// predates this task, re-asserted at the end so the permissive cell above reads as
/// a narrowing of the guard and not as its removal.
#[tokio::test]
async fn a_target_holding_any_platform_role_grant_is_refused() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let users = UserRepo::new(db.clone());
    let sessions = egret_auth::session::SessionService::new(
        db.clone(),
        chrono::Duration::hours(8),
        chrono::Duration::hours(1),
    );
    let admin = seed_user(&db).await;

    // Row 1: an ordinary member with no platform grant of any kind, under BOTH
    // scopes. Nothing about impersonation changed for this target, and if it had,
    // the rest of the test would be meaningless.
    let plain = seed_user(&db).await;
    for scope in [None, Some(egret_core::WorkspaceId(uuid::Uuid::new_v4()))] {
        egret_auth::impersonation::validate_impersonation_target(
            &users, &sessions, admin, plain, "debugging", scope,
        )
        .await
        .unwrap_or_else(|e| {
            panic!("a target with no platform role grant must be impersonable at scope {scope:?}: {e:?}")
        });
    }

    // Row 2: the escalating target. A platform-scoped role carrying
    // `policy:update` — the shape migration 0059's `data_steward` has, built here
    // with its own role so the test does not depend on the seed's contents.
    let target = seed_user(&db).await;
    let role_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) VALUES ('platform', $1, 'task 13') \
         RETURNING id",
    )
    .bind(format!("task13_steward_{}", uuid::Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, 'policy:update')")
        .bind(role_id)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL)",
    )
    .bind(target.0)
    .bind(role_id)
    .execute(&db.pool)
    .await
    .unwrap();

    // The fixture holds the state it guards: the target's OWN session resolves to
    // a principal carrying `policy:update`, which is the authority a scoped
    // overlay over this user would hand a `/api/control-plane` write route.
    let issued = sessions.create(target, "test", "127.0.0.1").await.unwrap();
    let resolved = sessions
        .validate(&issued.token)
        .await
        .unwrap()
        .expect("the target's own session resolves");
    assert!(
        resolved.permissions.iter().any(|p| p == "policy:update"),
        "fixture does not hold the state under test — the target must really carry \
         policy:update, got {:?}",
        resolved.permissions
    );
    assert!(
        !resolved
            .platform_roles
            .iter()
            .any(|r| r == "platform_admin"),
        "the target must NOT be a platform_admin, or the pre-Task-13 guard would \
         have refused it and this test would prove nothing; roles: {:?}",
        resolved.platform_roles
    );

    // Row 2, ws-scoped: REFUSED. This is the cell that closes the chain.
    let err = egret_auth::impersonation::validate_impersonation_target(
        &users,
        &sessions,
        admin,
        target,
        "debugging",
        Some(egret_core::WorkspaceId(uuid::Uuid::new_v4())),
    )
    .await
    .expect_err(
        "a ws-scoped overlay must not be minted over a holder of platform authority: \
         no /api/control-plane route confines one",
    );
    assert!(
        matches!(err, egret_core::Error::Forbidden),
        "expected Forbidden, got: {err:?}"
    );

    // Row 2, platform-scoped: PERMITTED, and this arm is what the fix round
    // restored. The caller on that path is already a `platform_admin`, who holds
    // strictly more than this target, so refusing it prevented no escalation and
    // removed a support path.
    egret_auth::impersonation::validate_impersonation_target(
        &users,
        &sessions,
        admin,
        target,
        "debugging",
        None,
    )
    .await
    .expect(
        "a platform-scoped overlay MAY target a platform-permission holder who is \
         not a platform_admin — a platform_admin already holds strictly more",
    );

    // Now make that same target a platform_admin: refused under BOTH scopes, which
    // is the rule that predates this task.
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, id, 'platform', NULL FROM roles \
         WHERE scope = 'platform' AND name = 'platform_admin'",
    )
    .bind(target.0)
    .execute(&db.pool)
    .await
    .unwrap();
    for scope in [None, Some(egret_core::WorkspaceId(uuid::Uuid::new_v4()))] {
        let err = egret_auth::impersonation::validate_impersonation_target(
            &users,
            &sessions,
            admin,
            target,
            "debugging",
            scope,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, egret_core::Error::Forbidden),
            "a platform_admin target must be refused at scope {scope:?}, got: {err:?}"
        );
    }
}

// ── HTTP endpoint tests: the platform-admin impersonation start/exit surface ──
//
// These drive the real routers (`auth_router` + `admin_impersonation_router`) over
// an in-process `AuthState`, exercising the guards that make this a security
// surface: no-nesting, platform-admin only, step-up re-auth, no-self, reason
// required, target-not-admin, and the `/me` impersonation state.
mod http {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use egret_auth::routes::{admin_impersonation_router, auth_router, AuthState};
    use egret_auth::session::SessionService;
    use egret_config::Config;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// Insert a local user with password "pw"; return its id and email.
    async fn seed_local(db: &Db) -> (UserId, String) {
        let repo = UserRepo::new(db.clone());
        let email = unique_email("http");
        let id = repo
            .insert_local(&email, "H", &hash_password("pw").unwrap())
            .await
            .unwrap();
        (id, email)
    }

    /// Grant the caller the platform `platform_admin` role directly (user grant).
    async fn grant_platform_admin(db: &Db, uid: UserId) {
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
             SELECT 'user', $1, id, 'platform', NULL FROM roles \
             WHERE scope = 'platform' AND name = 'platform_admin' \
             ON CONFLICT DO NOTHING",
        )
        .bind(uid.0)
        .execute(&db.pool)
        .await
        .unwrap();
    }

    fn app(db: &Db) -> axum::Router {
        let st = AuthState {
            users: UserRepo::new(db.clone()),
            sessions: SessionService::new(
                db.clone(),
                chrono::Duration::hours(24),
                chrono::Duration::minutes(60),
            ),
            config: Config::default(),
            rate_limiter: egret_core::ratelimit::RateLimiter::new(50, 300, 300),
            audit: egret_audit::AuditSink::new(16).0,
            impersonations: ImpersonationRepo::new(db.clone()),
            oidc: None,
            cookie_key: None,
        };
        axum::Router::new()
            .nest("/api/auth", auth_router())
            .nest("/api/admin", admin_impersonation_router())
            .with_state(st)
    }

    /// Log in and return `(cookie_header, csrf_value)` — the cookie header carries
    /// both the session and CSRF cookies; the csrf value is echoed in the header
    /// for mutating requests.
    async fn login(app: &axum::Router, email: &str) -> (String, String) {
        let body =
            serde_json::to_vec(&serde_json::json!({"email": email, "password": "pw"})).unwrap();
        let resp = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "login should succeed");
        let mut session_kv = String::new();
        let mut csrf_kv = String::new();
        let mut csrf_val = String::new();
        for v in resp.headers().get_all("set-cookie").iter() {
            let s = v.to_str().unwrap();
            let kv = s.split(';').next().unwrap();
            if let Some(rest) = kv.strip_prefix("egret_session=") {
                session_kv = format!("egret_session={rest}");
            } else if let Some(rest) = kv.strip_prefix("egret_csrf=") {
                csrf_kv = format!("egret_csrf={rest}");
                csrf_val = rest.to_string();
            }
        }
        assert!(!session_kv.is_empty(), "session cookie present");
        assert!(!csrf_val.is_empty(), "csrf cookie present");
        (format!("{session_kv}; {csrf_kv}"), csrf_val)
    }

    async fn me(app: &axum::Router, cookie: &str) -> serde_json::Value {
        let resp = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/auth/me")
                    .header("cookie", cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// POST /api/admin/impersonate with the given JSON body; returns the status.
    async fn post_impersonate(
        app: &axum::Router,
        cookie: &str,
        csrf: &str,
        body: serde_json::Value,
    ) -> StatusCode {
        let resp = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/admin/impersonate")
                    .header("cookie", cookie)
                    .header("x-egret-csrf", csrf)
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        resp.status()
    }

    async fn active_overlay_count(db: &Db, impersonator: UserId) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM impersonations WHERE impersonator_id = $1",
        )
        .bind(impersonator.0)
        .fetch_one(&db.pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn platform_admin_impersonates_and_me_reflects_it() {
        let Some(db) = connect().await else {
            eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
            return;
        };
        let app = app(&db);
        let (admin, admin_email) = seed_local(&db).await;
        grant_platform_admin(&db, admin).await;
        let (target, target_email) = seed_local(&db).await;
        let (cookie, csrf) = login(&app, &admin_email).await;

        let status = post_impersonate(
            &app,
            &cookie,
            &csrf,
            serde_json::json!({
                "target_user_id": target.0.to_string(),
                "reason": "support ticket 42",
                "password": "pw",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let m = me(&app, &cookie).await;
        assert_eq!(
            m["email"].as_str().unwrap().to_lowercase(),
            target_email.to_lowercase(),
            "me now resolves to the target"
        );
        let imp = &m["impersonating"];
        assert!(!imp.is_null(), "impersonating state present");
        assert_eq!(
            imp["target"]["email"].as_str().unwrap().to_lowercase(),
            target_email.to_lowercase()
        );
        assert_eq!(
            imp["impersonator"]["user_id"].as_str().unwrap(),
            admin.0.to_string()
        );
    }

    #[tokio::test]
    async fn wrong_step_up_password_is_401_and_writes_nothing() {
        let Some(db) = connect().await else {
            eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
            return;
        };
        let app = app(&db);
        let (admin, admin_email) = seed_local(&db).await;
        grant_platform_admin(&db, admin).await;
        let (target, _te) = seed_local(&db).await;
        let (cookie, csrf) = login(&app, &admin_email).await;

        let status = post_impersonate(
            &app,
            &cookie,
            &csrf,
            serde_json::json!({
                "target_user_id": target.0.to_string(),
                "reason": "nope",
                "password": "WRONG",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        // No overlay row was written for this admin.
        assert_eq!(
            active_overlay_count(&db, admin).await,
            0,
            "a wrong step-up password must write zero overlay rows"
        );
        // And me still resolves to the admin.
        let m = me(&app, &cookie).await;
        assert_eq!(
            m["email"].as_str().unwrap().to_lowercase(),
            admin_email.to_lowercase()
        );
        assert!(m["impersonating"].is_null());
    }

    #[tokio::test]
    async fn cannot_impersonate_a_platform_admin() {
        let Some(db) = connect().await else {
            eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
            return;
        };
        let app = app(&db);
        let (admin, admin_email) = seed_local(&db).await;
        grant_platform_admin(&db, admin).await;
        let (target, _te) = seed_local(&db).await;
        grant_platform_admin(&db, target).await; // target is also an admin
        let (cookie, csrf) = login(&app, &admin_email).await;

        let status = post_impersonate(
            &app,
            &cookie,
            &csrf,
            serde_json::json!({
                "target_user_id": target.0.to_string(),
                "reason": "x",
                "password": "pw",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(active_overlay_count(&db, admin).await, 0);
    }

    #[tokio::test]
    async fn cannot_impersonate_self() {
        let Some(db) = connect().await else {
            eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
            return;
        };
        let app = app(&db);
        let (admin, admin_email) = seed_local(&db).await;
        grant_platform_admin(&db, admin).await;
        let (cookie, csrf) = login(&app, &admin_email).await;

        let status = post_impersonate(
            &app,
            &cookie,
            &csrf,
            serde_json::json!({
                "target_user_id": admin.0.to_string(),
                "reason": "x",
                "password": "pw",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn reason_is_required() {
        let Some(db) = connect().await else {
            eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
            return;
        };
        let app = app(&db);
        let (admin, admin_email) = seed_local(&db).await;
        grant_platform_admin(&db, admin).await;
        let (target, _te) = seed_local(&db).await;
        let (cookie, csrf) = login(&app, &admin_email).await;

        let status = post_impersonate(
            &app,
            &cookie,
            &csrf,
            serde_json::json!({
                "target_user_id": target.0.to_string(),
                "reason": "   ",
                "password": "pw",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(active_overlay_count(&db, admin).await, 0);
    }

    #[tokio::test]
    async fn cannot_nest() {
        let Some(db) = connect().await else {
            eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
            return;
        };
        let app = app(&db);
        let (admin, admin_email) = seed_local(&db).await;
        grant_platform_admin(&db, admin).await;
        let (t1, _t1e) = seed_local(&db).await;
        let (t2, _t2e) = seed_local(&db).await;
        let (cookie, csrf) = login(&app, &admin_email).await;

        // First impersonation succeeds.
        assert_eq!(
            post_impersonate(
                &app,
                &cookie,
                &csrf,
                serde_json::json!({
                    "target_user_id": t1.0.to_string(),
                    "reason": "first",
                    "password": "pw",
                }),
            )
            .await,
            StatusCode::OK
        );
        // A second, while impersonating, is forbidden (no nesting). Note the
        // session now resolves to the target, so `password` is irrelevant — the
        // no-nest guard fires first.
        assert_eq!(
            post_impersonate(
                &app,
                &cookie,
                &csrf,
                serde_json::json!({
                    "target_user_id": t2.0.to_string(),
                    "reason": "second",
                    "password": "pw",
                }),
            )
            .await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn exit_returns_to_admin() {
        let Some(db) = connect().await else {
            eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
            return;
        };
        let app = app(&db);
        let (admin, admin_email) = seed_local(&db).await;
        grant_platform_admin(&db, admin).await;
        let (target, target_email) = seed_local(&db).await;
        let (cookie, csrf) = login(&app, &admin_email).await;

        assert_eq!(
            post_impersonate(
                &app,
                &cookie,
                &csrf,
                serde_json::json!({
                    "target_user_id": target.0.to_string(),
                    "reason": "x",
                    "password": "pw",
                }),
            )
            .await,
            StatusCode::OK
        );
        // Sanity: me is the target while impersonating.
        let m = me(&app, &cookie).await;
        assert_eq!(
            m["email"].as_str().unwrap().to_lowercase(),
            target_email.to_lowercase()
        );

        // Exit.
        let resp = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/auth/impersonation/exit")
                    .header("cookie", &cookie)
                    .header("x-egret-csrf", &csrf)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // me is the admin again, impersonating null.
        let m = me(&app, &cookie).await;
        assert_eq!(
            m["email"].as_str().unwrap().to_lowercase(),
            admin_email.to_lowercase()
        );
        assert!(m["impersonating"].is_null());
        assert_eq!(active_overlay_count(&db, admin).await, 0);
    }

    // ── Task 8: OIDC step-up ─────────────────────────────────────────────────
    //
    // An OIDC-provider admin calling start (with a valid session) must get a 409
    // carrying an authorize URL and code "oidc_step_up" — NOT a 400 — and must
    // write NO overlay. The real IdP round-trip (redirect → callback → overlay) is
    // proven by the Task 12 e2e, not here; this only asserts the 409 contract. We
    // stand up a tiny in-process mock IdP (discovery + JWKS) purely so the only
    // `OidcService` constructor (live discovery) succeeds — the same technique the
    // back-channel-logout HTTP test uses.

    /// Discover a real `OidcService` against a throwaway in-process mock IdP.
    async fn discover_mock_oidc() -> egret_auth::oidc::OidcService {
        use egret_machine_idp::generate_rsa_key;
        use serde_json::json;

        let km = generate_rsa_key("mock-kid").expect("rsa key");
        let jwk_value = km.public_jwk.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let base = format!("http://{addr}");
        let issuer = base.clone();
        let discovery = json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{base}/auth"),
            "token_endpoint": format!("{base}/token"),
            "jwks_uri": format!("{base}/jwks"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["RS256"],
        });
        let jwks_doc = json!({ "keys": [jwk_value] });
        let mock = axum::Router::new()
            .route(
                "/.well-known/openid-configuration",
                axum::routing::get(move || {
                    let d = discovery.clone();
                    async move { axum::Json(d) }
                }),
            )
            .route(
                "/jwks",
                axum::routing::get(move || {
                    let j = jwks_doc.clone();
                    async move { axum::Json(j) }
                }),
            );
        tokio::spawn(async move {
            axum::serve(listener, mock).await.expect("mock idp serve");
        });

        egret_auth::oidc::OidcService::discover(&egret_config::OidcConfig {
            enabled: true,
            issuer_url: issuer,
            client_id: "egret-backend".into(),
            client_secret: "mock-secret".into(),
            redirect_url: format!("{base}/api/auth/oidc/callback"),
            scopes: vec!["openid".into()],
            groups_claim: "groups".into(),
            post_login_redirect: "/".into(),
            catalog_audience: "catalog".into(),
        })
        .await
        .expect("discover against mock idp")
    }

    /// Build an app whose `AuthState` has OIDC wired (so the step-up 409 path is
    /// reachable), sharing `db` so a directly-created session validates.
    fn app_with_oidc(db: &Db, oidc: egret_auth::oidc::OidcService) -> axum::Router {
        let st = AuthState {
            users: UserRepo::new(db.clone()),
            sessions: SessionService::new(
                db.clone(),
                chrono::Duration::hours(24),
                chrono::Duration::minutes(60),
            ),
            config: Config::default(),
            rate_limiter: egret_core::ratelimit::RateLimiter::new(50, 300, 300),
            audit: egret_audit::AuditSink::new(16).0,
            impersonations: ImpersonationRepo::new(db.clone()),
            oidc: Some(oidc),
            cookie_key: Some(axum_extra::extract::cookie::Key::generate()),
        };
        axum::Router::new()
            .nest("/api/auth", auth_router())
            .nest("/api/admin", admin_impersonation_router())
            .with_state(st)
    }

    #[tokio::test]
    async fn oidc_admin_start_returns_409_step_up() {
        let Some(db) = connect().await else {
            eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
            return;
        };
        let oidc = discover_mock_oidc().await;
        let app = app_with_oidc(&db, oidc);

        // Seed a platform_admin whose provider = "oidc", and log them in by creating
        // a session directly (OIDC login needs a live IdP; we only need the cookie).
        let users = UserRepo::new(db.clone());
        let sub = format!("oidc-sub-{}", uuid::Uuid::new_v4());
        let admin = users
            .insert_oidc(&sub, &unique_email("oidc-admin"), "OA")
            .await
            .unwrap();
        grant_platform_admin(&db, admin).await;
        let (target, _te) = seed_local(&db).await;

        let sessions = SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        );
        let issued = sessions.create(admin, "", "").await.unwrap();
        let cookie = format!("egret_session={}; egret_csrf=csrf1", issued.token);

        let resp = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/api/admin/impersonate")
                    .header("cookie", &cookie)
                    .header("x-egret-csrf", "csrf1")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "target_user_id": target.0.to_string(),
                            "reason": "support ticket 8",
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::CONFLICT,
            "OIDC admin must get 409, not 400"
        );
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["code"], "oidc_step_up");
        let authorize_url = body["authorize_url"].as_str().unwrap_or("");
        assert!(
            !authorize_url.is_empty(),
            "authorize_url must be present and non-empty, got {body:?}"
        );
        // The step-up must force a fresh IdP credential prompt, not reuse an
        // existing SSO session — otherwise it isn't a re-authentication at all.
        assert!(
            authorize_url.contains("prompt=login"),
            "authorize_url must carry prompt=login, got {authorize_url}"
        );

        // No overlay was written — the overlay is created only by the callback.
        assert_eq!(
            active_overlay_count(&db, admin).await,
            0,
            "the 409 step-up must not write an overlay"
        );
    }

    // ── Task 8: OIDC callback security paths ─────────────────────────────────
    //
    // These exercise `complete_impersonation_step_up` — the callback's core — at
    // its boundary, passing the *already-resolved* authenticated user id (what the
    // real callback obtains from `provision_and_sync` after verifying the token).
    // This faithfully tests the same-admin check and the TOCTOU authority re-check
    // without standing up a full mock token-exchange (a signed id_token that lines
    // up nonce/PKCE/aud through `oidc.finish` needs a real IdP — see
    // `oidc_keycloak.rs`). The end-to-end code→token→finish round-trip is covered
    // by the Task-12 e2e.

    /// Build an `OidcRoutesState` sharing `db` (with a throwaway mock OIDC service;
    /// `complete_impersonation_step_up` never touches `oidc`, but the field is
    /// required to construct the state).
    async fn oidc_routes_state(db: &Db) -> egret_auth::oidc_routes::OidcRoutesState {
        egret_auth::oidc_routes::OidcRoutesState {
            oidc: discover_mock_oidc().await,
            users: UserRepo::new(db.clone()),
            groups: egret_auth::groups::GroupRepo::new(db.clone()),
            sessions: SessionService::new(
                db.clone(),
                chrono::Duration::hours(24),
                chrono::Duration::minutes(60),
            ),
            config: Config::default(),
            cookie_key: axum_extra::extract::cookie::Key::generate(),
            impersonations: ImpersonationRepo::new(db.clone()),
            audit: egret_audit::AuditSink::new(16).0,
            group_recompiler: std::sync::Arc::new(crate::recorder::Recorder::default()),
            invites: std::sync::Arc::new(crate::recorder::NoInvites),
        }
    }

    /// Seed an OIDC user (unique sub + email); return its id.
    async fn seed_oidc_user(db: &Db, tag: &str) -> UserId {
        UserRepo::new(db.clone())
            .insert_oidc(
                &format!("{tag}-sub-{}", uuid::Uuid::new_v4()),
                &unique_email(tag),
                "O",
            )
            .await
            .unwrap()
    }

    async fn call_step_up(
        st: &egret_auth::oidc_routes::OidcRoutesState,
        authenticated: UserId,
        intent: egret_auth::oidc_cookie::ImpersonateIntent,
    ) -> axum::response::Response {
        let jar = axum_extra::extract::cookie::PrivateCookieJar::new(
            axum_extra::extract::cookie::Key::generate(),
        );
        egret_auth::oidc_routes::complete_impersonation_step_up(st, jar, authenticated, intent)
            .await
    }

    /// The single most important property: a DIFFERENT admin completing the
    /// step-up (fresh auth resolves to B, but the sealed intent named A) is
    /// rejected — 403, and NO overlay for either party.
    #[tokio::test]
    async fn oidc_callback_rejects_different_admin_and_writes_nothing() {
        let Some(db) = connect().await else {
            eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
            return;
        };
        let st = oidc_routes_state(&db).await;
        let a = seed_oidc_user(&db, "adminA").await;
        grant_platform_admin(&db, a).await;
        let b = seed_oidc_user(&db, "adminB").await;
        let (target, _te) = seed_local(&db).await;

        let intent = egret_auth::oidc_cookie::ImpersonateIntent {
            impersonator_id: a.0, // sealed initiator = A
            target_user_id: target.0,
            reason: "support ticket".into(),
            workspace_id: None,
        };
        // Complete as B — must be rejected.
        let resp = call_step_up(&st, b, intent).await;
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "a different admin must not complete A's intent"
        );
        assert_eq!(active_overlay_count(&db, a).await, 0);
        assert_eq!(active_overlay_count(&db, b).await, 0);
    }

    /// TOCTOU: the same admin completes the step-up, but their `platform_admin`
    /// grant was revoked between start and callback — 403, no overlay.
    #[tokio::test]
    async fn oidc_callback_rejects_revoked_platform_admin() {
        let Some(db) = connect().await else {
            eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
            return;
        };
        let st = oidc_routes_state(&db).await;
        // `a` is the initiator AND the authenticated user (same admin), but is NOT
        // (or no longer) a platform_admin — the callback re-check must fail closed.
        let a = seed_oidc_user(&db, "revoked").await;
        let (target, _te) = seed_local(&db).await;

        let intent = egret_auth::oidc_cookie::ImpersonateIntent {
            impersonator_id: a.0,
            target_user_id: target.0,
            reason: "support ticket".into(),
            workspace_id: None,
        };
        let resp = call_step_up(&st, a, intent).await;
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "a revoked platform_admin must not complete the step-up"
        );
        assert_eq!(active_overlay_count(&db, a).await, 0);
    }

    /// Grant `uid` the workspace role `role` on `ws` (direct user grant).
    async fn grant_ws_role(db: &Db, uid: UserId, ws: WorkspaceId, role: &str) {
        let n = sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
             SELECT 'user', $1, id, 'workspace', $2 FROM roles \
             WHERE scope = 'workspace' AND name = $3 \
             ON CONFLICT DO NOTHING",
        )
        .bind(uid.0)
        .bind(ws.0)
        .bind(role)
        .execute(&db.pool)
        .await
        .unwrap()
        .rows_affected();
        assert_eq!(
            n, 1,
            "the fixture must really grant {role} on the workspace"
        );
    }

    /// **The OIDC step-up callback omitted `ws_impersonate_start`'s step 4**
    /// (whole-branch review, M-1).
    ///
    /// `ws_impersonate_start` runs FOUR target guards before writing a ws-scoped
    /// overlay: the three shared ones in `validate_impersonation_target`, and then a
    /// workspace-specific pair — the target must be a MEMBER of that workspace and
    /// must not be a `ws_admin` of it. An OIDC caller never reaches that write: the
    /// start endpoint returns a `409` and the overlay is minted here, in the
    /// callback — which re-checked the CALLER (`ws_admin_of`) and re-ran the shared
    /// target guards but never the workspace-specific pair. Remove the target from
    /// the workspace, or promote them to `ws_admin` there, during the IdP round trip
    /// and the overlay was minted anyway.
    ///
    /// Three cells, because a guard proven only by its refusals is not proven:
    /// non-member → 403, `ws_admin` there → 403, ordinary member → overlay minted.
    #[tokio::test]
    async fn oidc_callback_rejects_a_ws_scoped_target_who_is_not_a_member() {
        let Some(db) = connect().await else {
            eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
            return;
        };
        let st = oidc_routes_state(&db).await;
        let a = seed_oidc_user(&db, "wsAdminNonMember").await;
        let ws = seed_workspace(&db, a).await;
        grant_ws_role(&db, a, ws, "ws_admin").await;
        // The target holds NO role in this workspace at callback time — removed
        // during the IdP round trip, or never a member at all.
        let (target, _te) = seed_local(&db).await;

        let intent = egret_auth::oidc_cookie::ImpersonateIntent {
            impersonator_id: a.0,
            target_user_id: target.0,
            reason: "support ticket".into(),
            workspace_id: Some(ws.0),
        };
        let resp = call_step_up(&st, a, intent).await;
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "a ws-scoped step-up must not mint an overlay over a NON-MEMBER of that \
             workspace — the start endpoint refuses it, so the callback must too"
        );
        assert_eq!(
            active_overlay_count(&db, a).await,
            0,
            "a refused step-up writes no overlay"
        );
    }

    /// The second half of the omitted guard: the target is a `ws_admin` of the
    /// intent's workspace (promoted during the IdP round trip). `ws_impersonate_start`
    /// refuses that — a ws_admin impersonating a peer ws_admin is a lateral move
    /// inside the workspace's own authority — and so must the callback.
    #[tokio::test]
    async fn oidc_callback_rejects_a_ws_scoped_target_who_is_a_ws_admin_there() {
        let Some(db) = connect().await else {
            eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
            return;
        };
        let st = oidc_routes_state(&db).await;
        let a = seed_oidc_user(&db, "wsAdminPeer").await;
        let ws = seed_workspace(&db, a).await;
        grant_ws_role(&db, a, ws, "ws_admin").await;
        let (target, _te) = seed_local(&db).await;
        grant_ws_role(&db, target, ws, "ws_admin").await;

        let intent = egret_auth::oidc_cookie::ImpersonateIntent {
            impersonator_id: a.0,
            target_user_id: target.0,
            reason: "support ticket".into(),
            workspace_id: Some(ws.0),
        };
        let resp = call_step_up(&st, a, intent).await;
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "a ws-scoped step-up must not mint an overlay over a ws_admin of that workspace"
        );
        assert_eq!(active_overlay_count(&db, a).await, 0);
    }

    /// **The live cell.** An ordinary member of the intent's workspace still
    /// completes: without this, both refusals above would pass against a guard that
    /// rejects every ws-scoped step-up.
    #[tokio::test]
    async fn oidc_callback_completes_a_ws_scoped_step_up_for_an_ordinary_member() {
        let Some(db) = connect().await else {
            eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
            return;
        };
        let st = oidc_routes_state(&db).await;
        let a = seed_oidc_user(&db, "wsAdminOk").await;
        let ws = seed_workspace(&db, a).await;
        grant_ws_role(&db, a, ws, "ws_admin").await;
        let (target, _te) = seed_local(&db).await;
        grant_ws_role(&db, target, ws, "ws_editor").await;

        let intent = egret_auth::oidc_cookie::ImpersonateIntent {
            impersonator_id: a.0,
            target_user_id: target.0,
            reason: "support ticket".into(),
            workspace_id: Some(ws.0),
        };
        let resp = call_step_up(&st, a, intent).await;
        assert_eq!(
            resp.status(),
            StatusCode::SEE_OTHER,
            "a ws-scoped step-up over an ordinary MEMBER must still complete"
        );
        assert_eq!(active_overlay_count(&db, a).await, 1);
        let scope: Option<uuid::Uuid> = sqlx::query_scalar(
            "SELECT workspace_scope_id FROM impersonations WHERE impersonator_id = $1",
        )
        .bind(a.0)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(
            scope,
            Some(ws.0),
            "the minted overlay is confined to the intent's workspace"
        );
    }

    /// Extract the (plaintext) `egret_session` value from a response's Set-Cookie.
    fn session_token_from_response(resp: &axum::response::Response) -> String {
        for v in resp.headers().get_all("set-cookie").iter() {
            let s = v.to_str().unwrap();
            let kv = s.split(';').next().unwrap();
            if let Some(rest) = kv.strip_prefix("egret_session=") {
                return rest.to_string();
            }
        }
        panic!("no egret_session set-cookie in response");
    }

    /// Happy path: the same admin, still a platform_admin, with a valid target →
    /// overlay created on a fresh admin session, and `/me` over that session
    /// resolves to the target with `impersonating` populated.
    #[tokio::test]
    async fn oidc_callback_same_admin_creates_overlay_and_me_shows_it() {
        let Some(db) = connect().await else {
            eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
            return;
        };
        let st = oidc_routes_state(&db).await;
        let a = seed_oidc_user(&db, "adminOk").await;
        grant_platform_admin(&db, a).await;
        let (target, target_email) = seed_local(&db).await;

        let intent = egret_auth::oidc_cookie::ImpersonateIntent {
            impersonator_id: a.0,
            target_user_id: target.0,
            reason: "support ticket".into(),
            workspace_id: None,
        };
        let resp = call_step_up(&st, a, intent).await;
        assert_eq!(
            resp.status(),
            StatusCode::SEE_OTHER,
            "a completed step-up redirects into the app"
        );
        // Exactly one overlay for A, pointed at the target.
        assert_eq!(active_overlay_count(&db, a).await, 1);
        let overlay_target: uuid::Uuid = sqlx::query_scalar(
            "SELECT target_user_id FROM impersonations WHERE impersonator_id = $1",
        )
        .bind(a.0)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(overlay_target, target.0);

        // The fresh admin session (carried by the redirect) now resolves to the
        // target under an overlay: `/me` shows impersonating.
        let token = session_token_from_response(&resp);
        let cookie = format!("egret_session={token}");
        let m = me(&app(&db), &cookie).await;
        assert_eq!(
            m["email"].as_str().unwrap().to_lowercase(),
            target_email.to_lowercase(),
            "me resolves to the target under the overlay"
        );
        assert!(
            !m["impersonating"].is_null(),
            "me must report the active impersonation"
        );
        assert_eq!(
            m["impersonating"]["impersonator"]["user_id"]
                .as_str()
                .unwrap(),
            a.0.to_string()
        );
    }
}
