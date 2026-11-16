//! SP5a: an owner lowers to every privilege on the securable and below, and is
//! an ordinary allow — a deny on the path is kept beside it (D6: "A deny
//! anywhere on the path beats any allow"; the concepts spec carves no owner
//! exemption). DB-gated: the owner is read back from the store before lowering.

use egret_metastore::Db;
use egret_perm::{
    list_owners_for_catalogs_via, owner_grant, set_owner_via, Action, Effect, EngineType, Grant,
    GrantOrigin, Principal, PrincipalKind, PrincipalSet, Scope,
};
use egret_perm_ir::audience::Directory;
use egret_perm_ir::ep::{EpInput, EpKind, Mechanism};
use egret_perm_ir::lower::grants::lower_grants;
use egret_perm_ir::statement::{Effect as IrEffect, Level, Privilege, Target};
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn ep(catalog: Uuid) -> EpInput {
    EpInput {
        profile_id: 1,
        profile_name: "p".into(),
        kind: EpKind {
            engine: EngineType::Trino,
            mechanism: Mechanism::Ranger,
        },
        engines: vec![],
        default_version: Err("unread".into()),
        default_image_level: None,
        classification_channel: false,
        catalogs: [(catalog, "sales".to_string())].into_iter().collect(),
    }
}

fn alice() -> Principal {
    Principal {
        kind: PrincipalKind::User,
        r#ref: "alice".into(),
    }
}

async fn seed_owned_schema(db: &Db, schema: &str) -> (Uuid, Grant) {
    let cat: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) \
         VALUES ($1, 'generic', '{}'::jsonb) RETURNING id",
    )
    .bind(format!(
        "oir_{}",
        &Uuid::new_v4().simple().to_string()[..12]
    ))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let mut conn = db.pool.acquire().await.unwrap();
    let scope = Scope {
        catalog_id: cat,
        schema: Some(schema.into()),
        table: None,
        column: None,
    };
    set_owner_via(&mut conn, &scope, &alice()).await.unwrap();
    let owners = list_owners_for_catalogs_via(&mut conn, &[cat])
        .await
        .unwrap();
    assert_eq!(owners.len(), 1);
    (cat, owner_grant(&owners[0]))
}

#[tokio::test]
async fn an_owner_lowers_to_every_privilege_on_the_securable_and_its_subtree() {
    let Some(db) = test_db().await else { return };
    let (cat, g) = seed_owned_schema(&db, "hr").await;
    let out = lower_grants(&ep(cat), &[g], &Directory::default()).unwrap();

    // First statement: every privilege, as one allow on the schema path — a
    // path statement covers the subtree (table, column) structurally.
    let s = &out.statements[0];
    assert_eq!(
        s.effect,
        IrEffect::Allow {
            privileges: vec![
                Privilege::Browse,
                Privilege::Read,
                Privilege::Write,
                Privilege::Create,
                Privilege::Manage
            ]
        }
    );
    let Target::Path(p) = &s.target else {
        panic!("not a path")
    };
    assert_eq!(p.schema, Level::Named("hr".into()));
    assert_eq!((&p.table, &p.column), (&Level::Unnamed, &Level::Unnamed));
    // Nothing is exempt from the owner line.
    assert!(s.audience.except.is_empty());
}

#[tokio::test]
async fn a_deny_on_the_path_is_kept_beside_the_owner_allow_not_exempted() {
    let Some(db) = test_db().await else { return };
    let (cat, owner) = seed_owned_schema(&db, "hr").await;
    let deny = Grant {
        id: Uuid::from_u128(7),
        effect: Effect::Deny,
        actions: vec![Action::Read],
        scope: Scope {
            catalog_id: cat,
            schema: Some("hr".into()),
            table: Some("people".into()),
            column: None,
        },
        principals: PrincipalSet {
            include: vec![alice()],
            exclude: vec![],
        },
        origin: GrantOrigin::Explicit,
    };
    let out = lower_grants(&ep(cat), &[owner, deny], &Directory::default()).unwrap();
    let denies: Vec<_> = out
        .statements
        .iter()
        .filter(|s| matches!(s.effect, IrEffect::Deny { .. }))
        .collect();
    assert_eq!(
        denies.len(),
        1,
        "the deny must survive; owner is not exempt"
    );
    assert!(denies[0].audience.except.is_empty());
}
