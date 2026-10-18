//! People & groups Task 3, spec §4.1: over one fixture, the users whose
//! `standsFor` holds an `onEngines` entry for a group, a role or a workspace
//! are exactly the usernames [`crate::members::expand_enumerated_principals`]
//! emits for a grant naming it.
//!
//! The one place the two differ by design is a group or workspace member, or
//! (since the 2026-10-18 design R5) a role holder, with no username: the compile MINTS one and names them, while `standsFor` says
//! `no_username` (nobody is named until that compile runs). The test pins
//! that difference to exactly those people.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use std::collections::BTreeSet;

use egret_metastore::Db;
use egret_perm::{
    Action, Effect, Grant, GrantOrigin, Principal, PrincipalKind, PrincipalSet, Scope,
};
use uuid::Uuid;

use crate::members::expand_enumerated_principals;
use crate::people::{read_user_via, NotOnEngines, UserPrincipal};

async fn test_db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn user(pool: &sqlx::PgPool, t: &str, n: &str, with_username: bool) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) VALUES ('local', $1, $2, $3) \
         RETURNING id",
    )
    .bind(format!("{n}_{t}@egret.local"))
    .bind(n)
    .bind(with_username.then(|| format!("{n}_{t}")))
    .fetch_one(pool)
    .await
    .expect("user")
}

async fn group(pool: &sqlx::PgPool, name: &str, members: &[Uuid]) -> Uuid {
    let id: Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("group");
    for m in members {
        sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
            .bind(m)
            .bind(id)
            .execute(pool)
            .await
            .expect("member");
    }
    id
}

async fn role(pool: &sqlx::PgPool, scope: &str, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ($1, $2) RETURNING id")
        .bind(scope)
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("role")
}

#[allow(clippy::too_many_arguments)]
async fn holds(
    pool: &sqlx::PgPool,
    principal_type: &str,
    principal_id: Uuid,
    role_id: Uuid,
    ws: Option<Uuid>,
    effect: &str,
    expired: bool,
) {
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, scope_id, effect, expires_at) \
         VALUES ($1, $2, $3, CASE WHEN $4::uuid IS NULL THEN 'platform' ELSE 'workspace' END, \
                 $4, $5, CASE WHEN $6 THEN now() - interval '1 hour' ELSE NULL END)",
    )
    .bind(principal_type)
    .bind(principal_id)
    .bind(role_id)
    .bind(ws)
    .bind(effect)
    .bind(expired)
    .execute(pool)
    .await
    .expect("access grant");
}

/// What a compile emits for an allow grant naming `p`.
async fn expanded(conn: &mut sqlx::PgConnection, p: Principal) -> BTreeSet<String> {
    let mut grants = vec![Grant {
        id: Uuid::new_v4(),
        effect: Effect::Allow,
        actions: vec![Action::Read],
        scope: Scope {
            catalog_id: Uuid::new_v4(),
            schema: None,
            table: None,
            column: None,
        },
        principals: PrincipalSet {
            include: vec![p],
            exclude: Vec::new(),
        },
        origin: GrantOrigin::Explicit,
    }];
    expand_enumerated_principals(conn, &mut grants, &mut [])
        .await
        .expect("expand");
    grants
        .remove(0)
        .principals
        .include
        .into_iter()
        .map(|p| {
            assert_eq!(p.kind, PrincipalKind::User, "expanded to users only");
            p.r#ref
        })
        .collect()
}

#[tokio::test]
async fn stands_for_on_engines_is_the_compilers_expansion() {
    let Some(db) = test_db().await else { return };
    let p = &db.pool;
    let t = Uuid::new_v4().simple().to_string()[..12].to_string();
    // Groups, roles, a workspace, expired rows, deny rows, group-held rows and
    // people with no username, in each place one can be.
    let ua = user(p, &t, "ua", true).await;
    let ub = user(p, &t, "ub", false).await; // g1 member, r1 holder, no username
    let uc = user(p, &t, "uc", true).await;
    let ud = user(p, &t, "ud", true).await;
    let ue = user(p, &t, "ue", true).await;
    let uf = user(p, &t, "uf", false).await; // W member, no username
    let g1 = group(p, &format!("g1_{t}"), &[ua, ub]).await;
    let g2 = group(p, &format!("g2_{t}"), &[uc]).await;
    group(p, &format!("g3_{t}"), &[]).await;
    let r1 = role(p, "platform", &format!("r1_{t}")).await;
    let r1w = role(p, "workspace", &format!("r1_{t}")).await; // same name, other scope
    let rw = role(p, "workspace", &format!("rw_{t}")).await;
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, $1) RETURNING id")
            .bind(format!("agree-{t}"))
            .fetch_one(p)
            .await
            .expect("workspace");
    holds(p, "user", ua, r1, None, "allow", false).await;
    holds(p, "user", ub, r1, None, "allow", false).await;
    holds(p, "user", ue, r1, None, "allow", true).await;
    holds(p, "user", uc, r1w, Some(ws), "allow", false).await;
    holds(p, "user", ud, r1, None, "deny", false).await;
    holds(p, "group", g2, r1, None, "allow", false).await;
    holds(p, "user", ud, rw, Some(ws), "allow", false).await;
    holds(p, "user", uf, rw, Some(ws), "allow", false).await;
    holds(p, "user", ua, rw, Some(ws), "allow", true).await;
    holds(p, "group", g1, rw, Some(ws), "allow", false).await;
    let _ = g2;

    let users = [ua, ub, uc, ud, ue, uf];
    let no_username: BTreeSet<Uuid> = [ub, uf].into_iter().collect();
    let mut conn = p.acquire().await.expect("conn");
    let mut read: Vec<UserPrincipal> = Vec::new();
    for u in users {
        read.push(
            read_user_via(&mut conn, u)
                .await
                .expect("read")
                .expect("exists"),
        );
    }
    drop(conn);

    let targets = [
        (PrincipalKind::Group, format!("g1_{t}")),
        (PrincipalKind::Group, format!("g2_{t}")),
        (PrincipalKind::Group, format!("g3_{t}")),
        (PrincipalKind::Role, format!("r1_{t}")),
        (PrincipalKind::Role, format!("rw_{t}")),
        (PrincipalKind::WorkspaceMembers, ws.to_string()),
    ];
    for (kind, r) in targets {
        let said: BTreeSet<String> = read
            .iter()
            .filter(|u| {
                u.stands_for
                    .iter()
                    .any(|s| s.kind == kind && s.r#ref == r && s.on_engines)
            })
            .map(|u| u.username.clone().expect("on engines implies a username"))
            .collect();

        // Rolled back, so the mint does not outlive the comparison.
        let mut tx = p.begin().await.expect("tx");
        let emitted = expanded(
            &mut tx,
            Principal {
                kind,
                r#ref: r.clone(),
            },
        )
        .await;
        let mut minted: Vec<(Uuid, String)> = Vec::new();
        for u in &no_username {
            let name: Option<String> =
                sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
                    .bind(u)
                    .fetch_one(&mut *tx)
                    .await
                    .expect("username");
            if let Some(name) = name {
                minted.push((*u, name));
            }
        }
        tx.rollback().await.expect("rollback");

        let minted_names: BTreeSet<String> = minted.iter().map(|(_, n)| n.clone()).collect();
        let named: BTreeSet<String> = emitted.difference(&minted_names).cloned().collect();
        assert_eq!(said, named, "{kind:?} {r}: standsFor vs the expansion");
        // Exactly the people with no username the compile minted are the
        // difference, and standsFor lists each of them, as `no_username`.
        for (u, name) in &minted {
            assert!(emitted.contains(name), "{kind:?} {r}: minted but not named");
            let s = read
                .iter()
                .find(|p| p.id == *u)
                .unwrap()
                .stands_for
                .iter()
                .find(|s| s.kind == kind && s.r#ref == r)
                .unwrap_or_else(|| panic!("{kind:?} {r}: {u} not listed"))
                .clone();
            assert!(!s.on_engines);
            assert_eq!(s.not_on_engines, Some(NotOnEngines::NoUsername));
        }
    }
}
